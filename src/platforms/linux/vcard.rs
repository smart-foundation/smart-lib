//! The Linux **vcard** cell: an in-process RFC 6350 parser and formatter over
//! the engine's own contact store.
//!
//! Native mechanism: this engine's own code, end to end. There is no
//! `libvcard` on this host, and there is no shellout (law #9): the contract's
//! `vcard.capnp` declares "Zero backend assumptions: no libvcard, no EKD, no
//! platform contact APIs. Backend provides vCard parsing/generation", and this
//! cell is that backend — the parsing, the generation and the store are all
//! the engine's own.
//!
//! What is real here:
//!
//! * **The parser** reads RFC 6350 content lines: groups, the open parameter
//!   list (quoted values included), line unfolding, the `\` escapes, structured
//!   values (`N`, `ADR`, `ORG`), base64 binary values (`PHOTO`, `LOGO`,
//!   `SOUND`, `KEY`), URI values, and dates.
//! * **The formatter** writes the same dialect back: `BEGIN:VCARD` /
//!   `VERSION` / `END:VCARD`, the property groups and parameters, the escapes,
//!   and folding at the requested length (RFC 6350's 75 octets by default).
//! * **The mapping** is the contract's normative table, row by row: the
//!   `VCardContact` ↔ `Contact` correspondence declared in `vcard.capnp`'s
//!   header, including the `TYPE`-token rules (a published token maps, an
//!   unrecognised one is preserved verbatim and ignored) and the E.164
//!   normalisation of phone numbers with the raw value preserved.
//! * **The store** is the engine's own memory: contacts, a monotonic revision
//!   counter, and the sync tokens derived from it.
//!
//! What is not served, and why: `resolveConflict` — this store has no remote
//! peer, so it never produces a conflict and there is nothing for the method
//! to resolve. It refuses rather than pretending.

extern crate std;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::core::{Error, Result};
use crate::domains::vcard::backend::{ConflictResolution, VcardBackend};
use crate::domains::vcard::types::{
    Contact, ContactAddress, ContactDate, ContactEmail, ContactPhone, ContactPhoto,
    DuplicateHandling, ExportRequest, ExportResult, FormatRequest, ImportRequest, ImportResult,
    ImportedContact, OutputFormat, PersonName, SyncResult, VCard, VCardParameter, VCardProperty,
    VCardValue, VCardVersion,
};

/// The property names whose value is a `;`-separated component list.
const STRUCTURED_PROPERTIES: &[&str] = &["N", "ADR", "ORG"];

/// The property names whose value may be inline binary.
const BINARY_PROPERTIES: &[&str] = &["PHOTO", "LOGO", "SOUND", "KEY"];

/// The property names whose value is a URI.
const URI_PROPERTIES: &[&str] = &["URL", "IMPP", "CALADRURI", "CALURI", "FBURL", "GEO"];

/// The property names whose value is a date.
const DATE_PROPERTIES: &[&str] = &["BDAY", "ANNIVERSARY"];

/// The published `TYPE` tokens of `contacts.capnp` (the phone, email and
/// address vocabularies).
const PHONE_TYPE_TOKENS: &[&str] = &["mobile", "home", "work", "fax", "other"];
const EMAIL_TYPE_TOKENS: &[&str] = &["home", "work", "other"];
const ADDRESS_TYPE_TOKENS: &[&str] = &["home", "work", "other"];

// ═══════════════════════════════════════════════════════════════════════
// The store
// ═══════════════════════════════════════════════════════════════════════

/// A stored contact and the revision it was last written at.
struct StoredContact {
    contact: Contact,
    revision: u64,
}

/// The engine's own contact store: contacts by id, a monotonic revision
/// counter, and the ids that were deleted.
#[derive(Default)]
struct Store {
    contacts: BTreeMap<u64, StoredContact>,
    next_id: u64,
    revision: u64,
    deleted: Vec<u64>,
}

impl Store {
    fn new() -> Self {
        Store {
            contacts: BTreeMap::new(),
            next_id: 1,
            revision: 0,
            deleted: Vec::new(),
        }
    }

    /// Write a contact and move the revision forward.
    fn put(&mut self, contact: Contact) -> (u64, u64) {
        self.revision += 1;
        let revision = self.revision;
        let id = contact.id;
        self.contacts.insert(
            id,
            StoredContact {
                contact,
                revision,
            },
        );
        (id, revision)
    }
}

/// The Linux vCard backend.
pub struct LinuxVcardBackend {
    store: Mutex<Store>,
}

impl LinuxVcardBackend {
    /// A backend with an empty store.
    pub fn new() -> Self {
        LinuxVcardBackend {
            store: Mutex::new(Store::new()),
        }
    }

    /// The contacts the store holds, in id order — the read half of the
    /// import/export pair, exposed for callers that want the store without
    /// going through the contract's export shape.
    pub fn contacts(&self) -> Result<Vec<Contact>> {
        let store = self.store.lock().map_err(|_| Error::failed("vcard store lock failed"))?;
        Ok(store.contacts.values().map(|c| c.contact.clone()).collect())
    }
}

impl Default for LinuxVcardBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl VcardBackend for LinuxVcardBackend {
    fn import_v_card(&self, request: ImportRequest) -> Result<ImportResult> {
        let cards = parse_cards(&request.data, request.version)?;
        let mut result = ImportResult {
            total_processed: cards.len() as u32,
            ..ImportResult::default()
        };

        let mut store = self.store.lock().map_err(|_| Error::failed("vcard store lock failed"))?;

        for card in &cards {
            let (mut contact, warnings) = card_to_contact(card);
            let uid = contact.metadata_value("vcard.uid").unwrap_or("").to_string();

            // The store's key is the card's own UID — the only identifier a
            // vCard carries that survives a round trip.
            let existing = if uid.is_empty() {
                None
            } else {
                store
                    .contacts
                    .iter()
                    .find(|(_, stored)| stored.contact.metadata_value("vcard.uid") == Some(uid.as_str()))
                    .map(|(id, stored)| (*id, stored.contact.clone()))
            };

            let handling = if request.merge {
                DuplicateHandling::Merge
            } else {
                request.duplicate_handling
            };

            match (existing, handling) {
                (Some((id, _stored)), DuplicateHandling::Skip) => {
                    result.skipped += 1;
                    result.contacts.push(ImportedContact {
                        contact_id: id,
                        vcard_uid: uid,
                        created: false,
                        warnings,
                    });
                }
                (Some((id, _stored)), DuplicateHandling::Update) => {
                    contact.id = id;
                    let (id, _) = store.put(contact);
                    result.imported += 1;
                    result.contacts.push(ImportedContact {
                        contact_id: id,
                        vcard_uid: uid,
                        created: false,
                        warnings,
                    });
                }
                (Some((_id, mut stored)), DuplicateHandling::Merge) => {
                    merge_contact(&mut stored, contact);
                    let (id, _) = store.put(stored);
                    result.imported += 1;
                    result.contacts.push(ImportedContact {
                        contact_id: id,
                        vcard_uid: uid,
                        created: false,
                        warnings,
                    });
                }
                (Some(_), DuplicateHandling::Create) => {
                    // `create` always makes a second contact, even though the
                    // key exists — that is what the contract's spelling says.
                    let id = store.next_id;
                    store.next_id += 1;
                    contact.id = id;
                    let (id, _) = store.put(contact);
                    result.imported += 1;
                    result.contacts.push(ImportedContact {
                        contact_id: id,
                        vcard_uid: uid,
                        created: true,
                        warnings,
                    });
                }
                (existing, _) => {
                    let id = existing.map(|(id, _)| id).unwrap_or_else(|| {
                        let id = store.next_id;
                        store.next_id += 1;
                        id
                    });
                    contact.id = id;
                    let (id, _) = store.put(contact);
                    result.imported += 1;
                    result.contacts.push(ImportedContact {
                        contact_id: id,
                        vcard_uid: uid,
                        created: true,
                        warnings,
                    });
                }
            }
        }

        // The warnings every card raised, once each, so a caller sees the
        // whole import's advice and not only the per-contact list.
        for imported in &result.contacts {
            for warning in &imported.warnings {
                if !result.warnings.contains(warning) {
                    result.warnings.push(warning.clone());
                }
            }
        }

        Ok(result)
    }

    fn export_v_card(&self, request: ExportRequest) -> Result<ExportResult> {
        let store = self.store.lock().map_err(|_| Error::failed("vcard store lock failed"))?;

        let mut contacts: Vec<Contact> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        if request.all_contacts || request.contact_ids.is_empty() {
            contacts.extend(store.contacts.values().map(|c| c.contact.clone()));
        } else {
            for id in &request.contact_ids {
                match store.contacts.get(id) {
                    Some(stored) => contacts.push(stored.contact.clone()),
                    None => errors.push(alloc::format!("no contact with id {id}")),
                }
            }
        }

        let version = if request.version == VCardVersion::Auto {
            VCardVersion::V40
        } else {
            request.version
        };

        let mut out = ExportResult {
            warnings: Vec::new(),
            errors,
            ..ExportResult::default()
        };

        // `single` is one card per output; `multiple` and `stream` are the
        // same bytes on the wire (a stream of cards *is* a sequence of cards),
        // and the difference is stated rather than faked.
        let cards: Vec<&Contact> = match request.output_format {
            OutputFormat::Single => contacts.iter().take(1).collect(),
            OutputFormat::Multiple | OutputFormat::Stream => contacts.iter().collect(),
        };
        if request.output_format == OutputFormat::Single && contacts.len() > 1 {
            out.warnings.push(alloc::format!(
                "outputFormat=single writes one card; {} contacts were requested",
                contacts.len()
            ));
        }

        for contact in cards {
            let (card, warnings) = contact_to_card(contact, version, request.inline_photos);
            for warning in warnings {
                if !out.warnings.contains(&warning) {
                    out.warnings.push(warning);
                }
            }
            out.data.extend(cards_to_text(&[card], version, request.line_length));
            out.exported += 1;
        }

        Ok(out)
    }

    fn parse_v_card(&self, data: &[u8], version: VCardVersion) -> Result<Vec<VCard>> {
        parse_cards(data, version)
    }

    fn format_v_card(&self, request: FormatRequest) -> Result<Vec<u8>> {
        let version = if request.version == VCardVersion::Auto {
            VCardVersion::V40
        } else {
            request.version
        };
        let (card, _warnings) = contact_to_card(&request.contact, version, request.inline_photos);
        Ok(cards_to_text(&[card], version, request.line_length))
    }

    fn get_supported_versions(&self) -> Result<Vec<VCardVersion>> {
        // What this backend can actually read and write — the three published
        // versions. The canonical tree deleted the hypothetical `v50` slot, so
        // there is no fourth entry to claim.
        Ok(VCardVersion::SUPPORTED.to_vec())
    }

    fn get_sync_token(&self, contact_ids: &[u64]) -> Result<String> {
        let store = self.store.lock().map_err(|_| Error::failed("vcard store lock failed"))?;
        let revision = if contact_ids.is_empty() {
            store.revision
        } else {
            let mut highest = 0;
            for id in contact_ids {
                let stored = store
                    .contacts
                    .get(id)
                    .ok_or_else(|| Error::not_found())?;
                highest = highest.max(stored.revision);
            }
            highest
        };
        Ok(alloc::format!("vcard-sync-{revision}"))
    }

    fn sync_changes(&self, sync_token: &str) -> Result<SyncResult> {
        let store = self.store.lock().map_err(|_| Error::failed("vcard store lock failed"))?;
        let since = parse_sync_token(sync_token)?;
        let mut changed = Vec::new();
        for stored in store.contacts.values() {
            if stored.revision > since {
                let (card, _warnings) = contact_to_card(
                    &stored.contact,
                    VCardVersion::V40,
                    true,
                );
                changed.push(card);
            }
        }
        Ok(SyncResult {
            new_sync_token: alloc::format!("vcard-sync-{}", store.revision),
            changed,
            deleted: store.deleted.clone(),
            has_more: false,
            conflicts: Vec::new(),
        })
    }

    fn resolve_conflict(&self, _conflict_id: &str, _resolution: ConflictResolution) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: vcard resolveConflict is not served — this store has no remote peer, so it never produces a conflict to resolve",
        ))
    }

    fn vcard_caps(&self) -> Vec<&'static str> {
        alloc::vec!["vcard.inproc", "vcard.rfc6350", "vcard.2.1", "vcard.3.0", "vcard.4.0"]
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The parser
// ═══════════════════════════════════════════════════════════════════════

/// One parsed content line.
#[derive(Clone, Debug, Default, PartialEq)]
struct ContentLine {
    group: String,
    name: String,
    parameters: Vec<VCardParameter>,
    value: String,
    line_number: u32,
}

/// Parse vCard bytes into cards.
///
/// `requested` is the version the caller asked for; `VCardVersion::Auto`
/// takes the version from each card's own `VERSION` property. A card whose
/// version is not one this engine speaks is an error, never a guess.
fn parse_cards(data: &[u8], requested: VCardVersion) -> Result<Vec<VCard>> {
    let text = match core::str::from_utf8(data) {
        Ok(text) => text,
        Err(error) => {
            return Err(Error::invalid_argument(alloc::format!(
                "vcard data is not valid UTF-8 (byte {})",
                error.valid_up_to()
            )))
        }
    };

    let lines = unfold_lines(text);
    let mut cards: Vec<VCard> = Vec::new();
    let mut current: Option<VCard> = None;
    let mut declared_version: Option<VCardVersion> = None;
    let mut seen_begin = false;

    for (number, line) in lines.iter().enumerate() {
        let line_number = number as u32 + 1;
        if line.trim().is_empty() {
            continue;
        }
        let parsed = parse_content_line(line, line_number)?;
        let name = parsed.name.as_str();

        match name {
            "BEGIN" => {
                if parsed.value.trim().eq_ignore_ascii_case("VCARD") {
                    if current.is_some() {
                        return Err(Error::invalid_argument(alloc::format!(
                            "vcard data has a second BEGIN:VCARD at line {line_number} before the first END:VCARD"
                        )));
                    }
                    seen_begin = true;
                    current = Some(VCard {
                        version: VCardVersion::V40,
                        properties: Vec::new(),
                        raw_lines: Vec::new(),
                    });
                    declared_version = None;
                } else {
                    return Err(Error::invalid_argument(alloc::format!(
                        "vcard data has BEGIN:{} at line {line_number}; only BEGIN:VCARD is a card",
                        parsed.value.trim()
                    )));
                }
            }
            "END" => {
                if !parsed.value.trim().eq_ignore_ascii_case("VCARD") {
                    return Err(Error::invalid_argument(alloc::format!(
                        "vcard data has END:{} at line {line_number}; only END:VCARD closes a card",
                        parsed.value.trim()
                    )));
                }
                let card = current.take().ok_or_else(|| {
                    Error::invalid_argument(alloc::format!(
                        "vcard data has END:VCARD at line {line_number} without a BEGIN:VCARD"
                    ))
                })?;
                let version = declared_version.unwrap_or(VCardVersion::V40);
                if requested != VCardVersion::Auto && version != requested {
                    return Err(Error::invalid_argument(alloc::format!(
                        "vcard card at line {line_number} declares version {} but {} was requested",
                        version.as_str(),
                        requested.as_str()
                    )));
                }
                let mut card = card;
                card.version = version;
                cards.push(card);
            }
            _ => {
                let card = current.as_mut().ok_or_else(|| {
                    Error::invalid_argument(alloc::format!(
                        "vcard data has the property {name} at line {line_number} outside a card"
                    ))
                })?;
                if name == "VERSION" {
                    let declared = VCardVersion::parse(parsed.value.trim()).ok_or_else(|| {
                        Error::invalid_argument(alloc::format!(
                            "vcard card declares the version {:?} at line {line_number}; this engine speaks 2.1, 3.0 and 4.0",
                            parsed.value.trim()
                        ))
                    })?;
                    declared_version = Some(declared);
                }
                let property = VCardProperty {
                    name: name.to_string(),
                    group: parsed.group.clone(),
                    parameters: parsed.parameters.clone(),
                    values: values_for(name, &parsed.parameters, &parsed.value),
                };
                card.properties.push(property);
                card.raw_lines.push(line.clone());
            }
        }
    }

    if current.is_some() {
        return Err(Error::invalid_argument(
            "vcard data ends inside a card: no END:VCARD",
        ));
    }
    if !seen_begin {
        return Err(Error::invalid_argument("vcard data has no BEGIN:VCARD"));
    }

    Ok(cards)
}

/// Split the text into logical lines, unfolding the continuation lines.
///
/// RFC 6350 §3.2: a folded line is a CRLF followed by a single space or tab;
/// the continuation is the rest of the line, and the fold is removed.
fn unfold_lines(text: &str) -> Vec<String> {
    let mut logical: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut have_current = false;
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        // A folded line continues the previous one: the fold itself (the
        // single space or tab) is removed and the rest is appended.
        if have_current && (raw.starts_with(' ') || raw.starts_with('\t')) {
            current.push_str(&raw[1..]);
            continue;
        }
        if have_current {
            logical.push(core::mem::take(&mut current));
        }
        current = raw.to_string();
        have_current = true;
    }
    if have_current {
        logical.push(current);
    }
    logical
}

/// Parse one content line into its group, name, parameters and value.
fn parse_content_line(line: &str, line_number: u32) -> Result<ContentLine> {
    let (name_part, value) = split_at_unquoted(line, ':').ok_or_else(|| {
        Error::invalid_argument(alloc::format!(
            "vcard line {line_number} has no ':' — it is not a content line: {line:?}"
        ))
    })?;

    let segments = split_at_unquoted_all(name_part, ';');
    let mut iter = segments.into_iter();
    let head = iter.next().unwrap_or_default();
    let (group, name) = match head.split_once('.') {
        Some((group, name)) => (group.to_string(), name.to_string()),
        None => (String::new(), head.to_string()),
    };

    if name.is_empty() {
        return Err(Error::invalid_argument(alloc::format!(
            "vcard line {line_number} has no property name: {line:?}"
        )));
    }

    let mut parameters = Vec::new();
    for segment in iter {
        if segment.is_empty() {
            continue;
        }
        let (name, value) = match segment.split_once('=') {
            Some((name, value)) => (name.trim(), value.trim()),
            None => (segment.trim(), ""),
        };
        if name.is_empty() {
            continue;
        }
        // A parameter value may be a quoted string and may hold a
        // comma-separated list (`TYPE=home,work`): each item is its own
        // parameter, and the vocabulary stays open.
        for item in split_at_unquoted_all(&unquote(value), ',') {
            parameters.push(VCardParameter {
                name: name.to_ascii_uppercase(),
                value: item.trim().to_string(),
            });
        }
    }

    Ok(ContentLine {
        group,
        name: name.to_ascii_uppercase(),
        parameters,
        value: value.to_string(),
        line_number,
    })
}

/// The typed values of one property.
fn values_for(name: &str, parameters: &[VCardParameter], raw: &str) -> Vec<VCardValue> {
    if STRUCTURED_PROPERTIES.contains(&name) {
        let components = split_unescaped(raw, ';');
        return vec![VCardValue::Structured(components)];
    }
    if BINARY_PROPERTIES.contains(&name) {
        let encoding = parameters
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case("ENCODING"))
            .map(|p| p.value.to_ascii_lowercase())
            .unwrap_or_default();
        if encoding == "b" || encoding == "base64" {
            match base64_decode(raw) {
                Some(bytes) => return vec![VCardValue::Binary(bytes)],
                None => return vec![VCardValue::Text(unescape_value(raw))],
            }
        }
        return vec![VCardValue::Uri(unescape_value(raw))];
    }
    if URI_PROPERTIES.contains(&name) {
        return vec![VCardValue::Uri(unescape_value(raw))];
    }
    if DATE_PROPERTIES.contains(&name) {
        return vec![VCardValue::Date(unescape_value(raw))];
    }
    split_unescaped(raw, ',')
        .into_iter()
        .map(VCardValue::Text)
        .collect()
}

/// Split at the first unquoted occurrence of the separator.
fn split_at_unquoted(text: &str, separator: char) -> Option<(&str, &str)> {
    let mut quoted = false;
    for (index, character) in text.char_indices() {
        match character {
            '"' => quoted = !quoted,
            c if c == separator && !quoted => return Some((&text[..index], &text[index + 1..])),
            _ => {}
        }
    }
    None
}

/// Split at every unquoted occurrence of the separator.
fn split_at_unquoted_all(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (index, character) in text.char_indices() {
        match character {
            '"' => quoted = !quoted,
            c if c == separator && !quoted => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Strip the quotes of a quoted parameter value.
fn unquote(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Split on the separator, honouring the `\` escapes, and unescape each part.
fn split_unescaped(text: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in text.chars() {
        if escaped {
            current.push('\\');
            current.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            c if c == separator => {
                parts.push(unescape_value(&current));
                current.clear();
            }
            c => current.push(c),
        }
    }
    if escaped {
        current.push('\\');
    }
    parts.push(unescape_value(&current));
    parts
}

/// Undo the `\` escapes of a text value.
fn unescape_value(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(escaped) => out.push(escaped),
            None => out.push('\\'),
        }
    }
    out
}

/// Apply the `\` escapes of a text value.
fn escape_value(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            ',' => out.push_str("\\,"),
            ';' => out.push_str("\\;"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// Decode base64 (the `ENCODING=b` / `ENCODING=BASE64` payloads).
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn symbol(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in input.bytes() {
        match byte {
            b'=' => break,
            b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => {}
        }
        let value = symbol(byte)?;
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Encode base64.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════
// The formatter
// ═══════════════════════════════════════════════════════════════════════

/// Write cards as vCard text.
///
/// `line_length` is the folding length in octets (0 = no folding, the
/// contract's own spelling).
fn cards_to_text(cards: &[VCard], version: VCardVersion, line_length: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for card in cards {
        out.extend_from_slice(b"BEGIN:VCARD\r\n");
        out.extend_from_slice(alloc::format!("VERSION:{}\r\n", version.as_str()).as_bytes());
        for property in &card.properties {
            let line = render_property(property);
            out.extend_from_slice(fold_line(&line, line_length).as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"END:VCARD\r\n");
    }
    out
}

/// Render one property as a content line (without the line break).
fn render_property(property: &VCardProperty) -> String {
    let mut line = String::new();
    if !property.group.is_empty() {
        line.push_str(&property.group);
        line.push('.');
    }
    line.push_str(&property.name);

    for parameter in &property.parameters {
        line.push(';');
        line.push_str(&parameter.name);
        line.push('=');
        let value = if parameter.value.contains(' ') || parameter.value.contains(';') {
            alloc::format!("\"{}\"", parameter.value)
        } else {
            parameter.value.clone()
        };
        line.push_str(&value);
    }

    line.push(':');

    let mut values = property.values.iter();
    let first = values.next();
    match first {
        Some(VCardValue::Structured(components)) => {
            let rendered: Vec<String> = components.iter().map(|c| escape_value(c)).collect();
            line.push_str(&rendered.join(";"));
            for value in values {
                line.push(';');
                line.push_str(&render_value(value));
            }
        }
        Some(value) => {
            line.push_str(&render_value(value));
            for value in values {
                line.push(',');
                line.push_str(&render_value(value));
            }
        }
        None => {}
    }
    line
}

/// Render one typed value.
fn render_value(value: &VCardValue) -> String {
    match value {
        VCardValue::Text(text) => escape_value(text),
        VCardValue::Structured(components) => components
            .iter()
            .map(|c| escape_value(c))
            .collect::<Vec<String>>()
            .join(";"),
        VCardValue::Binary(bytes) => base64_encode(bytes),
        VCardValue::Uri(uri) => escape_value(uri),
        VCardValue::Date(date) => date.clone(),
        VCardValue::Boolean(value) => value.to_string(),
        VCardValue::Integer(value) => value.to_string(),
        VCardValue::Float(value) => alloc::format!("{value}"),
        VCardValue::Custom(bytes) => base64_encode(bytes),
    }
}

/// Fold a content line at `line_length` octets (0 = no folding).
fn fold_line(line: &str, line_length: u16) -> String {
    if line_length == 0 {
        return line.to_string();
    }
    let limit = line_length as usize;
    let mut out = String::with_capacity(line.len() + 8);
    let mut chunk = String::new();
    let mut chunk_len = 0usize;
    let mut first = true;
    for character in line.chars() {
        let width = character.len_utf8();
        // A continuation line spends one octet on the leading space, so the
        // first line may hold one octet more than the continuations.
        let budget = if first { limit } else { limit.saturating_sub(1) };
        if chunk_len + width > budget && !chunk.is_empty() {
            out.push_str(&chunk);
            out.push_str("\r\n ");
            chunk.clear();
            chunk_len = 1;
            first = false;
        }
        chunk.push(character);
        chunk_len += width;
    }
    out.push_str(&chunk);
    out
}

// ═══════════════════════════════════════════════════════════════════════
// The mapping — the contract's normative table, row by row
// ═══════════════════════════════════════════════════════════════════════

/// Map a parsed card to a contact, exactly as `vcard.capnp`'s header table
/// declares, and report every row that could not be honoured as a warning.
fn card_to_contact(card: &VCard) -> (Contact, Vec<String>) {
    let mut contact = Contact::default();
    let mut warnings: Vec<String> = Vec::new();

    // VCardContact.uid -> Contact.metadata["vcard.uid"]
    if let Some(uid) = card.first_text("UID") {
        if !uid.is_empty() {
            contact.set_metadata("vcard.uid", uid);
        }
    }

    // VCardContact.fn -> Contact.name.display
    if let Some(display) = card.first_text("FN") {
        contact.name.display = display.to_string();
    }

    // VCardContact.n -> Contact.name.{family,given,middle,prefix,suffix}
    // (empty components are omitted, not blanked)
    if let Some(property) = card.property("N") {
        if let Some(VCardValue::Structured(components)) = property.values.first() {
            let get = |index: usize| components.get(index).map(|s| s.as_str()).unwrap_or("");
            let family = get(0);
            let given = get(1);
            let additional = get(2);
            let prefix = get(3);
            let suffix = get(4);
            if !family.is_empty() {
                contact.name.family = family.to_string();
            }
            if !given.is_empty() {
                contact.name.given = given.to_string();
            }
            if !additional.is_empty() {
                contact.name.middle = additional.to_string();
            }
            if !prefix.is_empty() {
                contact.name.prefix = prefix.to_string();
            }
            if !suffix.is_empty() {
                contact.name.suffix = suffix.to_string();
            }
        }
    }

    // VCardContact.nickname -> Contact.metadata["vcard.nickname"]
    if let Some(nickname) = card.first_text("NICKNAME") {
        if !nickname.is_empty() {
            contact.set_metadata("vcard.nickname", nickname);
        }
    }

    // VCardContact.org -> Contact.organization.{name,department}
    // and Contact.type = "organization" when FN is the org name
    if let Some(property) = card.property("ORG") {
        if let Some(VCardValue::Structured(components)) = property.values.first() {
            let name = components.first().map(|s| s.as_str()).unwrap_or("");
            let units: Vec<&str> = components.iter().skip(1).map(|s| s.as_str()).collect();
            if !name.is_empty() {
                contact.organization.name = name.to_string();
            }
            let department = units
                .iter()
                .filter(|unit| !unit.is_empty())
                .copied()
                .collect::<Vec<&str>>()
                .join(" ");
            if !department.is_empty() {
                contact.organization.department = department;
            }
            if !name.is_empty() && contact.name.display == name {
                contact.type_ = "organization".to_string();
            }
        }
    }

    // VCardContact.title -> Contact.organization.title — the canonical tree's
    // `T.OrganizationName` has no `title` field, so the value is preserved
    // in the metadata and the gap is reported rather than dropped.
    if let Some(title) = card.first_text("TITLE") {
        if !title.is_empty() {
            contact.set_metadata("vcard.title", title);
            warnings.push(alloc::format!(
                "the contract's mapping row `VCardContact.title -> Contact.organization.title` has no destination field: `T.OrganizationName` carries name, department, role and industry but no title, so the title is preserved in metadata[\"vcard.title\"]"
            ));
        }
    }

    // VCardContact.role -> Contact.organization.role
    if let Some(role) = card.first_text("ROLE") {
        if !role.is_empty() {
            contact.organization.role = role.to_string();
        }
    }

    // VCardContact.tel -> Contact.phones[].value (E.164 normalised when
    // parseable; the raw value is preserved)
    for property in card.properties_named("TEL") {
        let raw = property
            .values
            .first()
            .and_then(|v| v.first_text())
            .unwrap_or("")
            .to_string();
        if raw.is_empty() {
            continue;
        }
        contact.phones.push(ContactPhone {
            type_: type_token(property, PHONE_TYPE_TOKENS),
            value: normalise_phone(&raw),
            raw: raw.clone(),
            primary: property
                .parameters_named("PREF")
                .first()
                .map(|v| v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("yes"))
                .unwrap_or(false),
        });
    }
    if let Some(raw) = contact.phones.first().map(|phone| phone.raw.clone()) {
        contact.set_metadata("vcard.tel.raw", &raw);
    }

    // VCardContact.email -> Contact.emails[].value
    for property in card.properties_named("EMAIL") {
        let value = property
            .values
            .first()
            .and_then(|v| v.first_text())
            .unwrap_or("")
            .to_string();
        if value.is_empty() {
            continue;
        }
        contact.emails.push(ContactEmail {
            type_: type_token(property, EMAIL_TYPE_TOKENS),
            value,
            primary: property
                .parameters_named("PREF")
                .first()
                .map(|v| v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("yes"))
                .unwrap_or(false),
        });
    }

    // VCardContact.adr -> Contact.addresses[] (street/city/region/postalCode/
    // country, in that component order)
    for property in card.properties_named("ADR") {
        let components = match property.values.first() {
            Some(VCardValue::Structured(components)) => components.clone(),
            _ => continue,
        };
        let get = |index: usize| components.get(index).map(|s| s.as_str()).unwrap_or("");
        let po_box = get(0);
        let extended = get(1);
        if !po_box.is_empty() || !extended.is_empty() {
            warnings.push(alloc::format!(
                "the ADR components the contract's mapping table does not name (PO box {:?}, extended address {:?}) were dropped; the table maps street, city, region, postal code and country only",
                po_box,
                extended
            ));
        }
        contact.addresses.push(ContactAddress {
            type_: type_token(property, ADDRESS_TYPE_TOKENS),
            street: get(2).to_string(),
            city: get(3).to_string(),
            region: get(4).to_string(),
            postal_code: get(5).to_string(),
            country: get(6).to_string(),
            primary: property
                .parameters_named("PREF")
                .first()
                .map(|v| v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("yes"))
                .unwrap_or(false),
        });
    }

    // VCardContact.photo -> Contact.photo (T.DataHandle)
    if let Some(property) = card.property("PHOTO") {
        match property.values.first() {
            Some(VCardValue::Binary(bytes)) => {
                contact.photo = Some(ContactPhoto {
                    data: bytes.clone(),
                    media_type: property
                        .parameter("TYPE")
                        .unwrap_or_default()
                        .to_string(),
                    uri: String::new(),
                });
            }
            Some(VCardValue::Uri(uri)) => {
                contact.photo = Some(ContactPhoto {
                    data: Vec::new(),
                    media_type: property
                        .parameter("TYPE")
                        .unwrap_or_default()
                        .to_string(),
                    uri: uri.clone(),
                });
            }
            _ => {}
        }
    }

    // VCardContact.bday -> Contact.birthday (T.Date; the time part is
    // dropped, never guessed)
    if let Some(birthday) = card.first_value("BDAY").map(|v| value_text(v)) {
        if let Some(date) = parse_date(&birthday) {
            contact.birthday = Some(date);
        } else {
            warnings.push(alloc::format!(
                "the BDAY value {birthday:?} is not a date this engine reads, so no birthday was set"
            ));
        }
    }

    // VCardContact.anniversary -> Contact.metadata["vcard.anniversary"]
    if let Some(anniversary) = card.first_value("ANNIVERSARY").map(|v| value_text(v)) {
        if !anniversary.is_empty() {
            contact.set_metadata("vcard.anniversary", &anniversary);
            contact.anniversary = parse_date(&anniversary);
        }
    }

    // VCardContact.categories -> Contact.tags
    for property in card.properties_named("CATEGORIES") {
        for value in property.values.iter().filter_map(|v| v.first_text()) {
            if !value.is_empty() {
                contact.tags.push(value.to_string());
            }
        }
    }

    // VCardContact.note -> Contact.note
    if let Some(note) = card.first_text("NOTE") {
        contact.note = note.to_string();
    }

    // VCardContact.geo -> Contact.customFields["geo"]
    if let Some(geo) = card.first_text("GEO") {
        if !geo.is_empty() {
            contact.set_custom_field("geo", geo);
        }
    }

    (contact, warnings)
}

/// Map a contact back to a card — the export half of the contract's table.
fn contact_to_card(
    contact: &Contact,
    version: VCardVersion,
    inline_photos: bool,
) -> (VCard, Vec<String>) {
    let mut card = VCard {
        version,
        properties: Vec::new(),
        raw_lines: Vec::new(),
    };
    let mut warnings: Vec<String> = Vec::new();

    let mut push = |name: &str, values: Vec<VCardValue>, parameters: Vec<VCardParameter>| {
        card.properties.push(VCardProperty {
            name: name.to_string(),
            group: String::new(),
            parameters,
            values,
        });
    };

    // FN — required by RFC 6350, and the display name the contract maps.
    let display = if contact.name.display.is_empty() {
        let mut parts = Vec::new();
        if !contact.name.given.is_empty() {
            parts.push(contact.name.given.clone());
        }
        if !contact.name.family.is_empty() {
            parts.push(contact.name.family.clone());
        }
        parts.join(" ")
    } else {
        contact.name.display.clone()
    };
    push("FN", vec![VCardValue::Text(display)], Vec::new());

    // N — the structured name, in the component order the RFC fixes.
    if !contact.name.family.is_empty()
        || !contact.name.given.is_empty()
        || !contact.name.middle.is_empty()
        || !contact.name.prefix.is_empty()
        || !contact.name.suffix.is_empty()
    {
        push(
            "N",
            vec![VCardValue::Structured(vec![
                contact.name.family.clone(),
                contact.name.given.clone(),
                contact.name.middle.clone(),
                contact.name.prefix.clone(),
                contact.name.suffix.clone(),
            ])],
            Vec::new(),
        );
    }

    // NICKNAME
    if let Some(nickname) = contact.metadata_value("vcard.nickname") {
        if !nickname.is_empty() {
            push("NICKNAME", vec![VCardValue::Text(nickname.to_string())], Vec::new());
        }
    }

    // ORG
    if !contact.organization.name.is_empty() || !contact.organization.department.is_empty() {
        let mut components = vec![contact.organization.name.clone()];
        if !contact.organization.department.is_empty() {
            components.push(contact.organization.department.clone());
        }
        push("ORG", vec![VCardValue::Structured(components)], Vec::new());
    }

    // TITLE — from the metadata the import preserved it in.
    if let Some(title) = contact.metadata_value("vcard.title") {
        if !title.is_empty() {
            push("TITLE", vec![VCardValue::Text(title.to_string())], Vec::new());
        }
    }

    // ROLE
    if !contact.organization.role.is_empty() {
        push(
            "ROLE",
            vec![VCardValue::Text(contact.organization.role.clone())],
            Vec::new(),
        );
    }

    // EMAIL
    for email in &contact.emails {
        let mut parameters = Vec::new();
        if !email.type_.is_empty() {
            parameters.push(VCardParameter {
                name: "TYPE".to_string(),
                value: email.type_.to_ascii_uppercase(),
            });
        }
        push(
            "EMAIL",
            vec![VCardValue::Text(email.value.clone())],
            parameters,
        );
    }

    // TEL
    for phone in &contact.phones {
        let mut parameters = Vec::new();
        if !phone.type_.is_empty() {
            parameters.push(VCardParameter {
                name: "TYPE".to_string(),
                value: phone.type_.to_ascii_uppercase(),
            });
        }
        push(
            "TEL",
            vec![VCardValue::Text(phone.value.clone())],
            parameters,
        );
    }

    // ADR — the seven components, in the order the RFC fixes.
    for address in &contact.addresses {
        let mut parameters = Vec::new();
        if !address.type_.is_empty() {
            parameters.push(VCardParameter {
                name: "TYPE".to_string(),
                value: address.type_.to_ascii_uppercase(),
            });
        }
        push(
            "ADR",
            vec![VCardValue::Structured(vec![
                String::new(),
                String::new(),
                address.street.clone(),
                address.city.clone(),
                address.region.clone(),
                address.postal_code.clone(),
                address.country.clone(),
            ])],
            parameters,
        );
    }

    // BDAY
    if let Some(birthday) = contact.birthday {
        if !birthday.is_empty() {
            push(
                "BDAY",
                vec![VCardValue::Date(format_date(&birthday, version))],
                Vec::new(),
            );
        }
    }

    // ANNIVERSARY
    if let Some(anniversary) = contact.anniversary {
        if !anniversary.is_empty() {
            push(
                "ANNIVERSARY",
                vec![VCardValue::Date(format_date(&anniversary, version))],
                Vec::new(),
            );
        }
    } else if let Some(raw) = contact.metadata_value("vcard.anniversary") {
        if !raw.is_empty() {
            push(
                "ANNIVERSARY",
                vec![VCardValue::Date(raw.to_string())],
                Vec::new(),
            );
        }
    }

    // CATEGORIES
    if !contact.tags.is_empty() {
        push(
            "CATEGORIES",
            contact
                .tags
                .iter()
                .map(|tag| VCardValue::Text(tag.clone()))
                .collect(),
            Vec::new(),
        );
    }

    // NOTE
    if !contact.note.is_empty() {
        push("NOTE", vec![VCardValue::Text(contact.note.clone())], Vec::new());
    }

    // GEO
    if let Some(geo) = contact.custom_field("geo") {
        if !geo.is_empty() {
            push("GEO", vec![VCardValue::Uri(geo.to_string())], Vec::new());
        }
    }

    // PHOTO
    if let Some(photo) = &contact.photo {
        let mut parameters = Vec::new();
        if !photo.media_type.is_empty() {
            // The media type is kept verbatim: a MIME type's case is not a
            // type *token*'s case, and `image/jpeg` must not become
            // `IMAGE/JPEG` on a round trip.
            parameters.push(VCardParameter {
                name: "TYPE".to_string(),
                value: photo.media_type.clone(),
            });
        }
        if inline_photos && !photo.data.is_empty() {
            parameters.push(VCardParameter {
                name: "ENCODING".to_string(),
                value: if version == VCardVersion::V40 {
                    "b".to_string()
                } else {
                    "BASE64".to_string()
                },
            });
            push(
                "PHOTO",
                vec![VCardValue::Binary(photo.data.clone())],
                parameters,
            );
        } else if !photo.uri.is_empty() {
            push("PHOTO", vec![VCardValue::Uri(photo.uri.clone())], parameters);
        }
    }

    // UID — vCard 2.1 has no UID property, so it is written for 3.0 and 4.0.
    if version != VCardVersion::V21 {
        if let Some(uid) = contact.metadata_value("vcard.uid") {
            if !uid.is_empty() {
                push("UID", vec![VCardValue::Text(uid.to_string())], Vec::new());
            }
        }
    }

    if version == VCardVersion::V21 && contact.metadata_value("vcard.uid").is_some() {
        warnings.push(
            "vCard 2.1 has no UID property, so the contact's UID was not written (it stays in the store)"
                .to_string(),
        );
    }

    (card, warnings)
}

/// The text of a value, whatever arm it carries.
fn value_text(value: &VCardValue) -> String {
    match value {
        VCardValue::Text(text) => text.clone(),
        VCardValue::Date(date) => date.clone(),
        VCardValue::Uri(uri) => uri.clone(),
        VCardValue::Structured(components) => components.join(";"),
        _ => String::new(),
    }
}

/// The `TYPE` token of a property, mapped to the published vocabulary when it
/// is one of them and preserved verbatim when it is not.
fn type_token(property: &VCardProperty, published: &[&str]) -> String {
    let token = match property.parameters_named("TYPE").first() {
        Some(token) => (*token).trim().to_string(),
        None => return String::new(),
    };
    if token.is_empty() {
        return String::new();
    }
    let lower = token.to_ascii_lowercase();
    if published.contains(&lower.as_str()) {
        lower
    } else {
        // Not a published token: preserved verbatim, and ignored by the
        // consumer rather than rejected.
        token
    }
}

/// Normalise a phone number to E.164 when it is parseable; otherwise the value
/// is returned as the card spelled it.
fn normalise_phone(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.starts_with('+') {
        return trimmed.to_string();
    }
    let mut digits = String::new();
    for character in trimmed.chars() {
        match character {
            '+' => {}
            '0'..='9' => digits.push(character),
            ' ' | '-' | '(' | ')' | '.' | '/' => {}
            _ => return trimmed.to_string(),
        }
    }
    if digits.is_empty() || digits.len() > 15 {
        return trimmed.to_string();
    }
    alloc::format!("+{digits}")
}

/// Parse a vCard date into its calendar parts.
fn parse_date(text: &str) -> Option<ContactDate> {
    let (year, month, day) = crate::domains::vcard::types::parse_vcard_date(text)?;
    Some(ContactDate {
        year: year as u32,
        month: month as u8,
        day: day as u8,
    })
}

/// The date text for a calendar day, in the dialect the version uses.
fn format_date(date: &ContactDate, version: VCardVersion) -> String {
    if version == VCardVersion::V21 {
        if date.month == 0 || date.day == 0 {
            alloc::format!("{:04}", date.year)
        } else {
            alloc::format!("{:04}{:02}{:02}", date.year, date.month, date.day)
        }
    } else if date.month == 0 || date.day == 0 {
        alloc::format!("{:04}", date.year)
    } else {
        alloc::format!("{:04}-{:02}-{:02}", date.year, date.month, date.day)
    }
}

/// Merge an imported contact into the stored one: the stored value wins where
/// it has one, the imported value fills the gaps.
fn merge_contact(stored: &mut Contact, imported: Contact) {
    if stored.name.display.is_empty() {
        stored.name.display = imported.name.display;
    }
    if stored.name.family.is_empty() {
        stored.name.family = imported.name.family;
    }
    if stored.name.given.is_empty() {
        stored.name.given = imported.name.given;
    }
    if stored.name.middle.is_empty() {
        stored.name.middle = imported.name.middle;
    }
    if stored.name.prefix.is_empty() {
        stored.name.prefix = imported.name.prefix;
    }
    if stored.name.suffix.is_empty() {
        stored.name.suffix = imported.name.suffix;
    }
    if stored.organization.name.is_empty() {
        stored.organization.name = imported.organization.name;
    }
    if stored.organization.department.is_empty() {
        stored.organization.department = imported.organization.department;
    }
    if stored.organization.role.is_empty() {
        stored.organization.role = imported.organization.role;
    }
    if stored.note.is_empty() {
        stored.note = imported.note;
    }
    if stored.type_.is_empty() {
        stored.type_ = imported.type_;
    }
    if stored.birthday.is_none() {
        stored.birthday = imported.birthday;
    }
    if stored.anniversary.is_none() {
        stored.anniversary = imported.anniversary;
    }
    if stored.photo.is_none() {
        stored.photo = imported.photo;
    }
    for email in imported.emails {
        if !stored.emails.iter().any(|e| e.value == email.value) {
            stored.emails.push(email);
        }
    }
    for phone in imported.phones {
        if !stored.phones.iter().any(|p| p.raw == phone.raw) {
            stored.phones.push(phone);
        }
    }
    for address in imported.addresses {
        if !stored
            .addresses
            .iter()
            .any(|a| a.street == address.street && a.city == address.city)
        {
            stored.addresses.push(address);
        }
    }
    for tag in imported.tags {
        if !stored.tags.contains(&tag) {
            stored.tags.push(tag);
        }
    }
    for (key, value) in imported.metadata {
        if stored.metadata_value(&key).is_none() {
            stored.set_metadata(&key, &value);
        }
    }
    for (key, value) in imported.custom_fields {
        if stored.custom_field(&key).is_none() {
            stored.set_custom_field(&key, &value);
        }
    }
}

/// The revision a sync token names.
fn parse_sync_token(token: &str) -> Result<u64> {
    let rest = token.strip_prefix("vcard-sync-").ok_or_else(|| {
        Error::invalid_argument(alloc::format!(
            "the sync token {token:?} is not one this backend issued"
        ))
    })?;
    rest.parse::<u64>().map_err(|_| {
        Error::invalid_argument(alloc::format!(
            "the sync token {token:?} does not carry a revision"
        ))
    })
}

// The domain's model of a person is the contract's `T.PersonName`; this alias
// keeps the import list at the top honest about what the mapping writes.
#[allow(dead_code)]
type MappedName = PersonName;

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::domains::vcard::types::{ExportRequest, FormatRequest, ImportRequest};

    const SAMPLE: &str = "BEGIN:VCARD\r\n\
VERSION:4.0\r\n\
FN:Ada Lovelace\r\n\
N:Lovelace;Ada;;;\r\n\
UID:ada-1\r\n\
EMAIL;TYPE=work:ada@example.org\r\n\
TEL;TYPE=CELL:+44 20 7946 0018\r\n\
ADR;TYPE=home:;;12 St James's Sq;London;;SW1;UK\r\n\
ORG:Analytical Engines;Research\r\n\
TITLE:Countess\r\n\
ROLE:Mathematician\r\n\
NOTE:First programmer\r\n\
CATEGORIES:maths,history\r\n\
BDAY:1815-12-10\r\n\
END:VCARD\r\n";

    /// `parseVCard` reads a card out of the bytes: the version the card
    /// declares, and the fields it carries.
    #[test]
    fn parse_v_card_reads_the_fields_the_bytes_carry() {
        let backend = LinuxVcardBackend::new();
        let cards = backend
            .parse_v_card(SAMPLE.as_bytes(), VCardVersion::Auto)
            .expect("parse");
        assert_eq!(cards.len(), 1, "one card, one parse");
        let card = &cards[0];
        assert_eq!(card.version, VCardVersion::V40, "the version the card declares");
        assert_eq!(card.first_text("FN"), Some("Ada Lovelace"));
        assert_eq!(card.first_text("UID"), Some("ada-1"));
        assert_eq!(
            card.first_text("EMAIL"),
            Some("ada@example.org"),
            "the card's own address"
        );
    }

    /// `formatVCard` writes a card back out: the bytes carry the fields the
    /// card holds, and they parse back to the same card.
    #[test]
    fn format_v_card_writes_the_fields_the_card_holds() {
        let backend = LinuxVcardBackend::new();
        let cards = backend
            .parse_v_card(SAMPLE.as_bytes(), VCardVersion::Auto)
            .expect("parse");
        let card = &cards[0];
        let text = cards_to_text(std::slice::from_ref(card), VCardVersion::V40, 0);
        let text = core::str::from_utf8(&text).expect("utf-8");
        assert!(text.starts_with("BEGIN:VCARD\r\nVERSION:4.0\r\n"));
        assert!(text.contains("FN:Ada Lovelace\r\n"));
        assert!(text.contains("N:Lovelace;Ada;;;\r\n"));
        assert!(text.contains("UID:ada-1\r\n"));
        assert!(text.ends_with("END:VCARD\r\n"));

        // The formatted card parses back to the same card.
        let again = backend
            .parse_v_card(text.as_bytes(), VCardVersion::V40)
            .expect("re-parse");
        assert_eq!(again[0].first_text("FN"), card.first_text("FN"));
        assert_eq!(again[0].first_text("UID"), card.first_text("UID"));
        assert_eq!(again[0].version, card.version);
    }

    #[test]
    fn a_folded_line_is_unfolded_and_an_escaped_value_is_unescaped() {
        let backend = LinuxVcardBackend::new();
        let folded = concat!(
            "BEGIN:VCARD\r\n",
            "VERSION:3.0\r\n",
            "FN:A very long name that the\r\n",
            "  writer folded across two lines\r\n",
            "NOTE:Semicolon\\; comma\\, backslash\\\\ done\r\n",
            "END:VCARD\r\n",
        );
        let cards = backend
            .parse_v_card(folded.as_bytes(), VCardVersion::Auto)
            .expect("parse");
        let card = &cards[0];
        assert_eq!(card.version, VCardVersion::V30);
        assert_eq!(
            card.first_text("FN"),
            Some("A very long name that the writer folded across two lines")
        );
        assert_eq!(
            card.first_text("NOTE"),
            Some("Semicolon; comma, backslash\\ done")
        );
    }

    #[test]
    fn a_structured_value_keeps_its_component_order() {
        let backend = LinuxVcardBackend::new();
        let cards = backend
            .parse_v_card(SAMPLE.as_bytes(), VCardVersion::Auto)
            .expect("parse");
        let property = cards[0].property("N").expect("N");
        assert_eq!(
            property.values.first().expect("value").as_components(),
            ["Lovelace", "Ada", "", "", ""]
        );
        let adr = cards[0].property("ADR").expect("ADR");
        assert_eq!(
            adr.values.first().expect("value").as_components(),
            ["", "", "12 St James's Sq", "London", "", "SW1", "UK"]
        );
    }

    #[test]
    fn an_import_maps_the_fields_the_contract_names() {
        let backend = LinuxVcardBackend::new();
        let result = backend
            .import_v_card(ImportRequest {
                data: SAMPLE.as_bytes().to_vec(),
                version: VCardVersion::Auto,
                ..ImportRequest::default()
            })
            .expect("import");
        assert_eq!(result.total_processed, 1);
        assert_eq!(result.imported, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.contacts.len(), 1);
        assert_eq!(result.contacts[0].vcard_uid, "ada-1");
        assert!(result.contacts[0].created);

        let contacts = backend.contacts().expect("contacts");
        let contact = &contacts[0];
        // uid -> metadata["vcard.uid"]
        assert_eq!(contact.metadata_value("vcard.uid"), Some("ada-1"));
        // fn -> name.display
        assert_eq!(contact.name.display, "Ada Lovelace");
        // n -> name.{family,given}
        assert_eq!(contact.name.family, "Lovelace");
        assert_eq!(contact.name.given, "Ada");
        // org -> organization.{name,department}
        assert_eq!(contact.organization.name, "Analytical Engines");
        assert_eq!(contact.organization.department, "Research");
        // role -> organization.role
        assert_eq!(contact.organization.role, "Mathematician");
        // title -> the contract's destination field does not exist: preserved
        // in the metadata and reported
        assert_eq!(contact.metadata_value("vcard.title"), Some("Countess"));
        assert!(result
            .warnings
            .iter()
            .any(|w| w.contains("organization.title")));
        // tel -> phones[].value, E.164 normalised, raw preserved
        assert_eq!(contact.phones.len(), 1);
        assert_eq!(contact.phones[0].value, "+442079460018");
        assert_eq!(contact.phones[0].raw, "+44 20 7946 0018");
        assert_eq!(contact.metadata_value("vcard.tel.raw"), Some("+44 20 7946 0018"));
        // an unpublished TYPE token is preserved verbatim
        assert_eq!(contact.phones[0].type_, "CELL");
        // email -> emails[].value, a published token maps to its spelling
        assert_eq!(contact.emails[0].value, "ada@example.org");
        assert_eq!(contact.emails[0].type_, "work");
        // adr -> addresses[]
        assert_eq!(contact.addresses[0].street, "12 St James's Sq");
        assert_eq!(contact.addresses[0].city, "London");
        assert_eq!(contact.addresses[0].postal_code, "SW1");
        assert_eq!(contact.addresses[0].country, "UK");
        // bday -> birthday
        assert_eq!(contact.birthday.expect("birthday").year, 1815);
        assert_eq!(contact.birthday.expect("birthday").month, 12);
        assert_eq!(contact.birthday.expect("birthday").day, 10);
        // categories -> tags
        assert_eq!(contact.tags, vec!["maths".to_string(), "history".to_string()]);
        // note -> note
        assert_eq!(contact.note, "First programmer");
    }

    #[test]
    fn an_export_writes_the_fields_the_contract_names() {
        let backend = LinuxVcardBackend::new();
        backend
            .import_v_card(ImportRequest {
                data: SAMPLE.as_bytes().to_vec(),
                version: VCardVersion::Auto,
                ..ImportRequest::default()
            })
            .expect("import");

        let contacts = backend.contacts().expect("contacts");
        let request = ExportRequest {
            all_contacts: true,
            output_format: OutputFormat::Multiple,
            ..ExportRequest::new()
        };
        let exported = backend.export_v_card(request).expect("export");
        assert_eq!(exported.exported, 1);
        let text = core::str::from_utf8(&exported.data).expect("utf-8");
        assert!(text.contains("FN:Ada Lovelace\r\n"));
        assert!(text.contains("N:Lovelace;Ada;;;\r\n"));
        assert!(text.contains("EMAIL;TYPE=WORK:ada@example.org\r\n"));
        assert!(text.contains("TEL;TYPE=CELL:+442079460018\r\n"));
        assert!(text.contains("ORG:Analytical Engines;Research\r\n"));
        assert!(text.contains("TITLE:Countess\r\n"));
        assert!(text.contains("ROLE:Mathematician\r\n"));
        assert!(text.contains("CATEGORIES:maths,history\r\n"));
        assert!(text.contains("BDAY:1815-12-10\r\n"));
        assert!(text.contains("UID:ada-1\r\n"));
        assert!(text.contains("ADR;TYPE=HOME:;;12 St James's Sq;London;;SW1;UK\r\n"));

        // The exported card parses back into the same contact fields.
        let reimported = backend
            .import_v_card(ImportRequest {
                data: exported.data.clone(),
                version: VCardVersion::Auto,
                ..ImportRequest::default()
            })
            .expect("re-import");
        assert_eq!(reimported.imported, 1);
        let contacts = backend.contacts().expect("contacts");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].name.display, "Ada Lovelace");
        assert_eq!(contacts[0].emails[0].value, "ada@example.org");
        assert_eq!(contacts[0].phones[0].value, "+442079460018");
    }

    #[test]
    fn the_supported_versions_are_the_ones_the_engine_can_read() {
        let backend = LinuxVcardBackend::new();
        let versions = backend.get_supported_versions().expect("versions");
        assert_eq!(versions, vec![VCardVersion::V21, VCardVersion::V30, VCardVersion::V40]);
        // Each of them parses: the claim is a measured one.
        for version in versions {
            let card = alloc::format!(
                "BEGIN:VCARD\r\nVERSION:{}\r\nFN:X\r\nEND:VCARD\r\n",
                version.as_str()
            );
            let cards = backend
                .parse_v_card(card.as_bytes(), version)
                .expect("parse");
            assert_eq!(cards[0].version, version);
        }
        // A version the engine does not speak is refused, never guessed.
        let unknown = b"BEGIN:VCARD\r\nVERSION:5.0\r\nFN:X\r\nEND:VCARD\r\n";
        let error = backend
            .parse_v_card(unknown, VCardVersion::Auto)
            .expect_err("a version this engine does not speak is an error");
        assert_eq!(error.code, crate::core::ErrorCode::InvalidArgument);
    }

    /// `getSyncToken` reports the engine's OWN change counter: the token
    /// moves when the engine's store changes, and not otherwise.
    #[test]
    fn get_sync_token_reports_the_engines_own_change_counter() {
        let backend = LinuxVcardBackend::new();
        let token = backend.get_sync_token(&[]).expect("token");
        assert_eq!(token, "vcard-sync-0", "a fresh engine has changed nothing");

        backend
            .import_v_card(ImportRequest {
                data: SAMPLE.as_bytes().to_vec(),
                version: VCardVersion::Auto,
                ..ImportRequest::default()
            })
            .expect("import");

        let token = backend.get_sync_token(&[]).expect("token");
        assert_eq!(token, "vcard-sync-1", "the counter moved with the import");
    }

    /// `syncChanges` hands back the changes since the token it was given —
    /// and refuses a token this backend did not issue, rather than
    /// interpreting it.
    #[test]
    fn sync_changes_hands_back_the_changes_since_the_token() {
        let backend = LinuxVcardBackend::new();
        backend
            .import_v_card(ImportRequest {
                data: SAMPLE.as_bytes().to_vec(),
                version: VCardVersion::Auto,
                ..ImportRequest::default()
            })
            .expect("import");

        let changes = backend.sync_changes("vcard-sync-0").expect("changes");
        assert_eq!(changes.changed.len(), 1, "the import is the one change");
        assert_eq!(changes.new_sync_token, "vcard-sync-1");
        assert!(changes.deleted.is_empty());
        assert!(!changes.has_more);

        // Nothing changed since the newest token.
        let changes = backend.sync_changes("vcard-sync-1").expect("changes");
        assert!(changes.changed.is_empty());

        // A token this backend did not issue is refused, not interpreted.
        let error = backend
            .sync_changes("some-other-token")
            .expect_err("a foreign token is refused");
        assert_eq!(error.code, crate::core::ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_conflict_is_refused_honestly() {
        let backend = LinuxVcardBackend::new();
        let error = backend
            .resolve_conflict("conflict-1", ConflictResolution::KeepLocal)
            .expect_err("there is no conflict surface to resolve");
        assert_eq!(error.code, crate::core::ErrorCode::NotSupported);
        assert!(error.message.contains("no remote peer"));
    }

    #[test]
    fn a_duplicate_is_handled_the_way_the_caller_asked() {
        let backend = LinuxVcardBackend::new();
        let import = |handling: DuplicateHandling| {
            backend
                .import_v_card(ImportRequest {
                    data: SAMPLE.as_bytes().to_vec(),
                    version: VCardVersion::Auto,
                    duplicate_handling: handling,
                    ..ImportRequest::default()
                })
                .expect("import")
        };
        let first = import(DuplicateHandling::Update);
        assert_eq!(first.imported, 1);
        assert!(first.contacts[0].created);

        let second = import(DuplicateHandling::Update);
        assert_eq!(second.imported, 1);
        assert!(!second.contacts[0].created);
        assert_eq!(backend.contacts().expect("contacts").len(), 1);

        let skipped = import(DuplicateHandling::Skip);
        assert_eq!(skipped.skipped, 1);
        assert_eq!(skipped.imported, 0);

        let created = import(DuplicateHandling::Create);
        assert!(created.contacts[0].created);
        assert_eq!(backend.contacts().expect("contacts").len(), 2);
    }

    #[test]
    fn a_photo_is_read_and_written_as_bytes() {
        let backend = LinuxVcardBackend::new();
        let card = "BEGIN:VCARD\r\n\
VERSION:4.0\r\n\
FN:Ada\r\n\
PHOTO;ENCODING=b;TYPE=image/jpeg:/9j/4AAQSkZJRg==\r\n\
END:VCARD\r\n";
        let cards = backend
            .parse_v_card(card.as_bytes(), VCardVersion::Auto)
            .expect("parse");
        let property = cards[0].property("PHOTO").expect("photo");
        match property.values.first().expect("value") {
            VCardValue::Binary(bytes) => {
                // "/9j/4AAQSkZJRg==" is the ten-byte JFIF header.
                assert_eq!(bytes, &[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46])
            }
            other => panic!("expected binary bytes, got {other:?}"),
        }

        let imported = backend
            .import_v_card(ImportRequest {
                data: card.as_bytes().to_vec(),
                version: VCardVersion::Auto,
                ..ImportRequest::default()
            })
            .expect("import");
        assert_eq!(imported.imported, 1);
        let contacts = backend.contacts().expect("contacts");
        let photo = contacts[0].photo.as_ref().expect("photo");
        assert_eq!(photo.data, vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46]);
        assert_eq!(photo.media_type, "image/jpeg");

        let exported = backend
            .export_v_card(ExportRequest {
                all_contacts: true,
                output_format: OutputFormat::Multiple,
                ..ExportRequest::new()
            })
            .expect("export");
        let text = core::str::from_utf8(&exported.data).expect("utf-8");
        assert!(text.contains("PHOTO;TYPE=image/jpeg;ENCODING=b:/9j/4AAQSkZJRg==\r\n"));
    }
}
