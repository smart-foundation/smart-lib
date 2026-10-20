//! The **vcard** domain's value types.
//!
//! Three models live here, and they are deliberately distinct:
//!
//! * [`VCard`] and friends — the **parsed** card (RFC 6350): the ordered
//!   property list, the open parameters, the typed values. This is what
//!   `parseVCard` returns and what `formatVCard` writes.
//! * [`Contact`] and friends — the **contact** model the contract's normative
//!   mapping table names (`contacts.capnp`'s `Contact`, mirrored here because
//!   the contacts schema is not compiled under the `domain-vcard` feature —
//!   `vcard.capnp` imports `types.capnp` and `calendar.capnp`, never
//!   `contacts.capnp`). Every field the table names is carried, and the one
//!   row whose destination field does not exist in the canonical tree
//!   (`Contact.organization.title` — `T.OrganizationName` has no `title`)
//!   is carried in the contact's metadata and reported as a warning rather
//!   than dropped.
//! * the request/result shapes the eight contract methods carry.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::schema::{types_capnp, vcard_capnp};

// ═══════════════════════════════════════════════════════════════════════
// The version vocabulary
// ═══════════════════════════════════════════════════════════════════════

/// The vCard versions this engine knows.
///
/// `Auto` is the contract's "detect from the card" spelling (`ImportRequest.version`
/// is "empty = auto-detect"); it is a *request*, never a version a card can
/// declare. The canonical tree deleted the hypothetical `v50` slot, so the
/// concrete set is exactly the three published versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VCardVersion {
    /// Detect the version from the card's own `VERSION` property.
    Auto,
    /// vCard 2.1 (the legacy interchange format).
    V21,
    /// vCard 3.0 (RFC 2425/2426).
    V30,
    /// vCard 4.0 (RFC 6350) — the latest published version.
    V40,
}

impl Default for VCardVersion {
    fn default() -> Self {
        VCardVersion::Auto
    }
}

impl VCardVersion {
    /// The version as the card spells it (`"2.1"`, `"3.0"`, `"4.0"`), or
    /// `"auto"` for the detection request.
    pub const fn as_str(self) -> &'static str {
        match self {
            VCardVersion::Auto => "auto",
            VCardVersion::V21 => "2.1",
            VCardVersion::V30 => "3.0",
            VCardVersion::V40 => "4.0",
        }
    }

    /// The concrete versions, in the order a caller should try them. `Auto`
    /// is not one of them: it is a request spelling, not a version.
    pub const SUPPORTED: &'static [VCardVersion] = &[VCardVersion::V21, VCardVersion::V30, VCardVersion::V40];

    /// Parse a card's `VERSION` value. `None` for a version this engine does
    /// not speak — the caller reports it, it is never guessed.
    pub fn parse(text: &str) -> Option<VCardVersion> {
        match text.trim() {
            "2.1" => Some(VCardVersion::V21),
            "3.0" => Some(VCardVersion::V30),
            "4.0" => Some(VCardVersion::V40),
            _ => None,
        }
    }

    /// The generated contract enum.
    pub const fn to_generated(self) -> vcard_capnp::VCardVersion {
        match self {
            VCardVersion::Auto => vcard_capnp::VCardVersion::Auto,
            VCardVersion::V21 => vcard_capnp::VCardVersion::V21,
            VCardVersion::V30 => vcard_capnp::VCardVersion::V30,
            VCardVersion::V40 => vcard_capnp::VCardVersion::V40,
        }
    }

    /// Read the generated contract enum back.
    pub fn from_generated(version: vcard_capnp::VCardVersion) -> VCardVersion {
        match version {
            vcard_capnp::VCardVersion::V21 => VCardVersion::V21,
            vcard_capnp::VCardVersion::V30 => VCardVersion::V30,
            vcard_capnp::VCardVersion::V40 => VCardVersion::V40,
            vcard_capnp::VCardVersion::Auto => VCardVersion::Auto,
            _ => VCardVersion::Auto,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The parsed card (RFC 6350)
// ═══════════════════════════════════════════════════════════════════════

/// One `NAME=VALUE` parameter of a content line.
///
/// The canonical tree deleted the frozen 1998 `TYPE` enum and replaced it
/// with this open key/value list, so an unknown parameter (a 2046 one) is
/// carried verbatim and never rejected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VCardParameter {
    /// The parameter name (`TYPE`, `ENCODING`, `PREF`, …), upper-cased.
    pub name: String,
    /// The parameter value, unquoted.
    pub value: String,
}

/// A typed value of a vCard property.
///
/// The union mirrors the contract's `VCardValue`: text, structured, binary,
/// uri, date, boolean, integer, float, custom.
#[derive(Clone, Debug, PartialEq)]
pub enum VCardValue {
    /// A text value.
    Text(String),
    /// A structured value: the `;`-separated components, in order.
    Structured(Vec<String>),
    /// Binary bytes (a base64-decoded `PHOTO`, `LOGO`, `SOUND`, `KEY`).
    Binary(Vec<u8>),
    /// A URI reference.
    Uri(String),
    /// A date, as the card spells it (`"1990-01-01"`, `"1990-01"`, `"1990"`).
    Date(String),
    /// A boolean.
    Boolean(bool),
    /// A signed integer.
    Integer(i64),
    /// A float.
    Float(f64),
    /// Bytes this engine does not interpret.
    Custom(Vec<u8>),
}

impl VCardValue {
    /// The text of a text value, or the empty string for any other arm.
    pub fn as_text(&self) -> &str {
        match self {
            VCardValue::Text(t) => t.as_str(),
            _ => "",
        }
    }

    /// The components of a structured value, or the empty slice.
    pub fn as_components(&self) -> &[String] {
        match self {
            VCardValue::Structured(components) => components.as_slice(),
            _ => &[],
        }
    }

    /// The first value of a property that carries text, if it carries any.
    pub fn first_text(&self) -> Option<&str> {
        match self {
            VCardValue::Text(t) => Some(t.as_str()),
            _ => None,
        }
    }
}

/// One content line of a card: an optional group, a name, open parameters and
/// one or more typed values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VCardProperty {
    /// The property name, upper-cased (`FN`, `N`, `EMAIL`, …).
    pub name: String,
    /// The property group (`item1`, `work`), empty when the line carries none.
    pub group: String,
    /// The open parameter list.
    pub parameters: Vec<VCardParameter>,
    /// The values, in order.
    pub values: Vec<VCardValue>,
}

impl VCardProperty {
    /// The value of the named parameter, if the property carries it.
    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| p.value.as_str())
    }

    /// Every value of the named parameter (a `TYPE=home,work` line carries two).
    pub fn parameters_named(&self, name: &str) -> Vec<&str> {
        self.parameters
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| p.value.as_str())
            .collect()
    }

    /// The property's text values, in order.
    pub fn texts(&self) -> Vec<&str> {
        self.values.iter().filter_map(|v| v.first_text()).collect()
    }
}

/// A parsed card.
#[derive(Clone, Debug, PartialEq)]
pub struct VCard {
    /// The version the card declares.
    pub version: VCardVersion,
    /// The properties, in the order the card lists them.
    pub properties: Vec<VCardProperty>,
    /// The raw content lines, in order — the round-trip channel for a
    /// property this engine does not model.
    pub raw_lines: Vec<String>,
}

impl Default for VCard {
    fn default() -> Self {
        VCard {
            version: VCardVersion::V40,
            properties: Vec::new(),
            raw_lines: Vec::new(),
        }
    }
}

impl VCard {
    /// Every property with the given name, in order.
    pub fn properties_named<'a, 'b>(&'a self, name: &'b str) -> impl Iterator<Item = &'a VCardProperty> + 'a
    where
        'b: 'a,
    {
        self.properties
            .iter()
            .filter(move |p| p.name.eq_ignore_ascii_case(name))
    }

    /// The first property with the given name.
    pub fn property<'a>(&'a self, name: &str) -> Option<&'a VCardProperty> {
        self.properties.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// The first text value of the named property.
    pub fn first_text(&self, name: &str) -> Option<&str> {
        self.property(name)
            .and_then(|p| p.values.first())
            .and_then(|v| v.first_text())
    }

    /// The first value of the named property, whatever arm it carries.
    pub fn first_value(&self, name: &str) -> Option<&VCardValue> {
        self.property(name).and_then(|p| p.values.first())
    }
}

// ── capnp conversions (the L1 half) ──────────────────────────────────

impl VCardParameter {
    /// Write into a generated `VCardParameter` builder.
    pub fn write_to(&self, builder: &mut vcard_capnp::v_card_parameter::Builder<'_>) {
        builder.set_name(&self.name);
        builder.set_value(&self.value);
    }

    /// Read from a generated `VCardParameter` reader.
    pub fn from_reader(reader: &vcard_capnp::v_card_parameter::Reader<'_>) -> Self {
        VCardParameter {
            name: text_of(reader.get_name()),
            value: text_of(reader.get_value()),
        }
    }
}

impl VCardValue {
    /// Write into a generated `VCardValue` builder.
    pub fn write_to(&self, builder: &mut vcard_capnp::v_card_value::Builder<'_>) {
        match self {
            VCardValue::Text(text) => {
                builder.set_text(text.as_str());
            }
            VCardValue::Structured(components) => {
                let mut list = builder.reborrow().init_structured().init_components(components.len() as u32);
                for (i, component) in components.iter().enumerate() {
                    list.set(i as u32, component.as_str());
                }
            }
            VCardValue::Binary(bytes) => {
                builder.set_binary(bytes.as_slice());
            }
            VCardValue::Uri(uri) => {
                write_uri(&mut builder.reborrow().init_uri(), uri);
            }
            VCardValue::Date(date) => {
                // The contract's `date` arm is a `T.Timestamp`, whose
                // nanosecond count is a `UInt64`: an instant before the Unix
                // epoch does not fit in it. A pre-epoch date is therefore
                // written as the two's-complement bit pattern of the signed
                // count, which keeps the round trip exact instead of
                // writing a date the card never carried. A date this engine
                // cannot read is written as the epoch itself — the one value
                // that is not a claim about the card's calendar day.
                let nanos = date_to_nanos(date).unwrap_or(0);
                builder.reborrow().init_date().set_nanoseconds(nanos as u64);
            }
            VCardValue::Boolean(value) => {
                builder.set_boolean(*value);
            }
            VCardValue::Integer(value) => {
                builder.set_integer(*value);
            }
            VCardValue::Float(value) => {
                builder.set_float(*value);
            }
            VCardValue::Custom(bytes) => {
                builder.set_custom(bytes.as_slice());
            }
        }
    }

    /// Read from a generated `VCardValue` reader.
    pub fn from_reader(reader: &vcard_capnp::v_card_value::Reader<'_>) -> Self {
        use vcard_capnp::v_card_value::WhichReader;
        match reader.which() {
            Ok(WhichReader::Text(Ok(text))) => VCardValue::Text(text_of(Ok(text))),
            Ok(WhichReader::Structured(Ok(structured))) => {
                let mut components = Vec::new();
                if let Ok(list) = structured.get_components() {
                    for component in list.iter() {
                        components.push(text_of(component));
                    }
                }
                VCardValue::Structured(components)
            }
            Ok(WhichReader::Binary(Ok(bytes))) => VCardValue::Binary(bytes.to_vec()),
            Ok(WhichReader::Uri(Ok(uri))) => VCardValue::Uri(read_uri(&uri)),
            Ok(WhichReader::Date(Ok(stamp))) => {
                VCardValue::Date(nanos_to_date(stamp.get_nanoseconds()))
            }
            Ok(WhichReader::Boolean(value)) => VCardValue::Boolean(value),
            Ok(WhichReader::Integer(value)) => VCardValue::Integer(value),
            Ok(WhichReader::Float(value)) => VCardValue::Float(value),
            Ok(WhichReader::Custom(Ok(bytes))) => VCardValue::Custom(bytes.to_vec()),
            _ => VCardValue::Text(String::new()),
        }
    }
}

impl VCardProperty {
    /// Write into a generated `VCardProperty` builder.
    pub fn write_to(&self, builder: &mut vcard_capnp::v_card_property::Builder<'_>) {
        builder.set_name(&self.name);
        builder.set_group(&self.group);
        let mut parameters = builder.reborrow().init_parameters(self.parameters.len() as u32);
        for (i, parameter) in self.parameters.iter().enumerate() {
            parameter.write_to(&mut parameters.reborrow().get(i as u32));
        }
        let mut values = builder.reborrow().init_values(self.values.len() as u32);
        for (i, value) in self.values.iter().enumerate() {
            value.write_to(&mut values.reborrow().get(i as u32));
        }
    }

    /// Read from a generated `VCardProperty` reader.
    pub fn from_reader(reader: &vcard_capnp::v_card_property::Reader<'_>) -> Self {
        let mut parameters = Vec::new();
        if let Ok(list) = reader.get_parameters() {
            for parameter in list.iter() {
                parameters.push(VCardParameter::from_reader(&parameter));
            }
        }
        let mut values = Vec::new();
        if let Ok(list) = reader.get_values() {
            for value in list.iter() {
                values.push(VCardValue::from_reader(&value));
            }
        }
        VCardProperty {
            name: text_of(reader.get_name()),
            group: text_of(reader.get_group()),
            parameters,
            values,
        }
    }
}

impl VCard {
    /// Write into a generated `VCard` builder.
    pub fn write_to(&self, builder: &mut vcard_capnp::v_card::Builder<'_>) {
        builder.set_version(self.version.to_generated());
        let mut properties = builder.reborrow().init_properties(self.properties.len() as u32);
        for (i, property) in self.properties.iter().enumerate() {
            property.write_to(&mut properties.reborrow().get(i as u32));
        }
        let mut raw = builder.reborrow().init_raw_lines(self.raw_lines.len() as u32);
        for (i, line) in self.raw_lines.iter().enumerate() {
            raw.set(i as u32, line.as_str());
        }
    }

    /// Read from a generated `VCard` reader.
    pub fn from_reader(reader: &vcard_capnp::v_card::Reader<'_>) -> Self {
        let mut properties = Vec::new();
        if let Ok(list) = reader.get_properties() {
            for property in list.iter() {
                properties.push(VCardProperty::from_reader(&property));
            }
        }
        let mut raw_lines = Vec::new();
        if let Ok(list) = reader.get_raw_lines() {
            for line in list.iter() {
                raw_lines.push(text_of(line));
            }
        }
        VCard {
            version: VCardVersion::from_generated(
                reader
                    .get_version()
                    .unwrap_or(vcard_capnp::VCardVersion::Unspecified),
            ),
            properties,
            raw_lines,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The contact model the contract's mapping table produces
// ═══════════════════════════════════════════════════════════════════════

/// A person's name — the contract's `T.PersonName` (`contacts.capnp`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PersonName {
    /// The given name.
    pub given: String,
    /// The family name.
    pub family: String,
    /// The middle name.
    pub middle: String,
    /// The name prefix (`Dr.`, `Ms.`).
    pub prefix: String,
    /// The name suffix (`Jr.`).
    pub suffix: String,
    /// The display name — the card's `FN`.
    pub display: String,
}

/// The organisation — the contract's `T.OrganizationName` (`contacts.capnp`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContactOrganization {
    /// The organisation's name (`ORG`'s first component).
    pub name: String,
    /// The department (`ORG`'s unit components).
    pub department: String,
    /// The role (`ROLE`).
    pub role: String,
    /// The industry — never carried by a vCard; always empty here.
    pub industry: String,
}

/// A phone number — the contract's `T.PhoneEntry`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContactPhone {
    /// The open type token (`home`, `work`, `mobile`, `fax`, `other`, or an
    /// unrecognised token preserved verbatim).
    pub type_: String,
    /// The normalised value (E.164 when parseable).
    pub value: String,
    /// The value exactly as the card spelled it — the contract requires the
    /// un-normalised value to be preserved.
    pub raw: String,
    /// Whether the card marked this entry preferred.
    pub primary: bool,
}

/// An email address — the contract's `T.EmailEntry`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContactEmail {
    /// The open type token (`home`, `work`, `other`, or an unrecognised one
    /// preserved verbatim).
    pub type_: String,
    /// The address.
    pub value: String,
    /// Whether the card marked this entry preferred.
    pub primary: bool,
}

/// A postal address — the contract's `T.AddressEntry`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContactAddress {
    /// The open type token (`home`, `work`, `other`, or an unrecognised one
    /// preserved verbatim).
    pub type_: String,
    /// The street component.
    pub street: String,
    /// The locality component.
    pub city: String,
    /// The region component.
    pub region: String,
    /// The postal-code component.
    pub postal_code: String,
    /// The country component.
    pub country: String,
    /// Whether the card marked this entry preferred.
    pub primary: bool,
}

/// A photo — the contract's `T.DataHandle` plus the media type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContactPhoto {
    /// The inline bytes (a base64-decoded `PHOTO`).
    pub data: Vec<u8>,
    /// The media type as the card spells it (`JPEG`, `image/jpeg`, …). The
    /// vCard `TYPE` parameter is a bare token, so it is kept verbatim; the
    /// wire form splits it into type/subtype when it carries a `/`.
    pub media_type: String,
    /// The URI, when the photo was a reference rather than inline bytes.
    pub uri: String,
}

/// A calendar day — the contract's `T.Date`.
///
/// A vCard date may be reduced-precision (`1990`, `1990-01`); the parts the
/// card did not state stay `0` (the wire default for "not stated"), never
/// guessed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContactDate {
    /// The year.
    pub year: u32,
    /// The month (1–12), or 0 when the card stated only the year.
    pub month: u8,
    /// The day (1–31), or 0 when the card stated no day.
    pub day: u8,
}

impl ContactDate {
    /// Whether the card stated nothing at all.
    pub fn is_empty(&self) -> bool {
        self.year == 0 && self.month == 0 && self.day == 0
    }
}

/// The contact model — the contract's `T.Contact` (`contacts.capnp`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contact {
    /// The contact's identifier, assigned by the store that holds it.
    pub id: u64,
    /// The person's name.
    pub name: PersonName,
    /// The email addresses.
    pub emails: Vec<ContactEmail>,
    /// The phone numbers.
    pub phones: Vec<ContactPhone>,
    /// The postal addresses.
    pub addresses: Vec<ContactAddress>,
    /// The photo.
    pub photo: Option<ContactPhoto>,
    /// The note.
    pub note: String,
    /// The birthday.
    pub birthday: Option<ContactDate>,
    /// The anniversary (the contract keeps it in the metadata).
    pub anniversary: Option<ContactDate>,
    /// The entity type as an open token (`organization` when the card's `FN`
    /// is its organisation's name).
    pub type_: String,
    /// The organisation.
    pub organization: ContactOrganization,
    /// The tags (`CATEGORIES`).
    pub tags: Vec<String>,
    /// The metadata map (`T.StringMap`), carried as ordered pairs.
    pub metadata: Vec<(String, String)>,
    /// The custom fields (`T.Contact.customFields`), carried as ordered pairs.
    pub custom_fields: Vec<(String, String)>,
}

impl Contact {
    /// The value of a metadata key.
    pub fn metadata_value(&self, key: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Set a metadata key (replacing an existing one).
    pub fn set_metadata(&mut self, key: &str, value: &str) {
        if let Some(entry) = self.metadata.iter_mut().find(|(k, _)| k == key) {
            entry.1 = value.to_string();
        } else {
            self.metadata.push((key.to_string(), value.to_string()));
        }
    }

    /// The value of a custom field.
    pub fn custom_field(&self, key: &str) -> Option<&str> {
        self.custom_fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Set a custom field (replacing an existing one).
    pub fn set_custom_field(&mut self, key: &str, value: &str) {
        if let Some(entry) = self.custom_fields.iter_mut().find(|(k, _)| k == key) {
            entry.1 = value.to_string();
        } else {
            self.custom_fields.push((key.to_string(), value.to_string()));
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The request/result shapes the eight contract methods carry
// ═══════════════════════════════════════════════════════════════════════

/// What to do with a contact that already exists — the contract's
/// `DuplicateHandling`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DuplicateHandling {
    /// Skip the duplicate (leave the stored contact alone).
    Skip,
    /// Update the stored contact with the imported one.
    Update,
    /// Create a second contact even though the key exists.
    Create,
    /// Merge the imported contact into the stored one.
    Merge,
}

impl Default for DuplicateHandling {
    fn default() -> Self {
        DuplicateHandling::Update
    }
}

impl DuplicateHandling {
    /// The generated contract enum.
    pub const fn to_generated(self) -> vcard_capnp::DuplicateHandling {
        match self {
            DuplicateHandling::Skip => vcard_capnp::DuplicateHandling::Skip,
            DuplicateHandling::Update => vcard_capnp::DuplicateHandling::Update,
            DuplicateHandling::Create => vcard_capnp::DuplicateHandling::Create,
            DuplicateHandling::Merge => vcard_capnp::DuplicateHandling::Merge,
        }
    }

    /// Read the generated contract enum back.
    pub fn from_generated(handling: vcard_capnp::DuplicateHandling) -> DuplicateHandling {
        match handling {
            vcard_capnp::DuplicateHandling::Skip => DuplicateHandling::Skip,
            vcard_capnp::DuplicateHandling::Update => DuplicateHandling::Update,
            vcard_capnp::DuplicateHandling::Create => DuplicateHandling::Create,
            vcard_capnp::DuplicateHandling::Merge => DuplicateHandling::Merge,
            _ => DuplicateHandling::Update,
        }
    }
}

/// The shape of one export — the contract's `OutputFormat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    /// One card per output.
    Single,
    /// Every card in one document.
    Multiple,
    /// Every card in one continuous document (the same bytes as `Multiple` on
    /// the vCard wire — a "stream" of cards *is* a sequence of cards).
    Stream,
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Single
    }
}

impl OutputFormat {
    /// The generated contract enum.
    pub const fn to_generated(self) -> vcard_capnp::OutputFormat {
        match self {
            OutputFormat::Single => vcard_capnp::OutputFormat::Single,
            OutputFormat::Multiple => vcard_capnp::OutputFormat::Multiple,
            OutputFormat::Stream => vcard_capnp::OutputFormat::Stream,
        }
    }

    /// Read the generated contract enum back.
    pub fn from_generated(format: vcard_capnp::OutputFormat) -> OutputFormat {
        match format {
            vcard_capnp::OutputFormat::Single => OutputFormat::Single,
            vcard_capnp::OutputFormat::Multiple => OutputFormat::Multiple,
            vcard_capnp::OutputFormat::Stream => OutputFormat::Stream,
            _ => OutputFormat::Single,
        }
    }
}

/// The contract's `ImportRequest`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportRequest {
    /// The vCard bytes.
    pub data: Vec<u8>,
    /// The version to parse with (`Auto` detects it from the card).
    pub version: VCardVersion,
    /// The group the imported contacts join (0 = no group).
    pub group_id: u64,
    /// What to do with a contact that already exists.
    pub duplicate_handling: DuplicateHandling,
    /// Whether an imported contact merges into an existing one.
    pub merge: bool,
    /// The caller's field-mapping overrides, keyed by the destination path
    /// the contract's table spells.
    pub field_mapping: Vec<(String, String)>,
}

/// The contract's `ImportedContact`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportedContact {
    /// The identifier the store assigned.
    pub contact_id: u64,
    /// The `UID` the card carried.
    pub vcard_uid: String,
    /// Whether the import created the contact (false = it updated an existing
    /// one).
    pub created: bool,
    /// The warnings this contact's import raised.
    pub warnings: Vec<String>,
}

/// The contract's `ImportError`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportError {
    /// The `UID` of the card that failed.
    pub vcard_uid: String,
    /// The line the failure was seen at (0 when it is not line-specific).
    pub line: u32,
    /// What went wrong.
    pub reason: String,
}

/// The contract's `ImportResult`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportResult {
    /// The imported contacts.
    pub contacts: Vec<ImportedContact>,
    /// How many cards were processed.
    pub total_processed: u32,
    /// How many contacts were imported.
    pub imported: u32,
    /// How many were skipped.
    pub skipped: u32,
    /// How many failed.
    pub failed: u32,
    /// The warnings raised by the whole import.
    pub warnings: Vec<String>,
    /// The per-card failures.
    pub errors: Vec<ImportError>,
}

/// The contract's `ExportRequest`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportRequest {
    /// The contacts to export (empty means "all", when `all_contacts` is set).
    pub contact_ids: Vec<u64>,
    /// Whether every stored contact is exported.
    pub all_contacts: bool,
    /// The version to write.
    pub version: VCardVersion,
    /// The property groups to include (empty = all).
    pub include_groups: Vec<String>,
    /// The properties to exclude.
    pub exclude_properties: Vec<String>,
    /// The caller's field-mapping overrides.
    pub field_mapping: Vec<(String, String)>,
    /// The folding length in octets (0 = no folding).
    pub line_length: u16,
    /// Whether a photo is written inline.
    pub inline_photos: bool,
    /// The output shape.
    pub output_format: OutputFormat,
}

impl ExportRequest {
    /// An export request with the contract's own defaults: vCard 4.0, one
    /// card, no folding limit beyond the RFC's 75 octets, inline photos.
    pub fn new() -> Self {
        ExportRequest {
            version: VCardVersion::V40,
            line_length: 75,
            inline_photos: true,
            output_format: OutputFormat::Single,
            ..ExportRequest::default()
        }
    }
}

/// The contract's `ExportSuccess` / `ExportPartial`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportResult {
    /// The vCard bytes.
    pub data: Vec<u8>,
    /// How many contacts were written.
    pub exported: u32,
    /// The warnings raised by the export.
    pub warnings: Vec<String>,
    /// The failures, when the export was partial.
    pub errors: Vec<String>,
}

/// The contract's `FormatRequest`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormatRequest {
    /// The contact to format.
    pub contact: Contact,
    /// The version to write.
    pub version: VCardVersion,
    /// The folding length in octets (0 = no folding).
    pub line_length: u16,
    /// Whether a photo is written inline.
    pub inline_photos: bool,
    /// The caller's field-mapping overrides.
    pub field_mapping: Vec<(String, String)>,
}

impl FormatRequest {
    /// A format request with the contract's own defaults: vCard 4.0 and the
    /// RFC's 75-octet folding.
    pub fn new(contact: Contact) -> Self {
        FormatRequest {
            contact,
            version: VCardVersion::V40,
            line_length: 75,
            inline_photos: true,
            field_mapping: Vec::new(),
        }
    }
}

/// The contract's `VCardSyncConflict`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncConflict {
    /// The conflict's identifier.
    pub conflict_id: String,
    /// The contact the conflict is about.
    pub contact_id: u64,
    /// The local card.
    pub local_version: Option<VCard>,
    /// The remote card.
    pub remote_version: Option<VCard>,
}

/// The contract's `VCardSyncResult`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncResult {
    /// The token to hand to the next `syncChanges`.
    pub new_sync_token: String,
    /// The cards that changed since the caller's token.
    pub changed: Vec<VCard>,
    /// The contacts deleted since the caller's token.
    pub deleted: Vec<u64>,
    /// Whether more changes are waiting.
    pub has_more: bool,
    /// The conflicts detected.
    pub conflicts: Vec<SyncConflict>,
}

// ═══════════════════════════════════════════════════════════════════════
// The pagination shape every domain in this tree carries
// ═══════════════════════════════════════════════════════════════════════

/// The pagination window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    /// The first row to return.
    pub offset: u32,
    /// How many rows to return (0 means one — never "everything").
    pub limit: u32,
}

impl Pagination {
    /// The effective limit: a zero limit means one row, never an unbounded
    /// read.
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 {
            1
        } else {
            self.limit
        }
    }
}

/// A page of rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    /// The rows.
    pub items: Vec<T>,
    /// How many rows the store holds in total.
    pub total: u64,
}

/// A subscription handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

// ═══════════════════════════════════════════════════════════════════════
// Small helpers
// ═══════════════════════════════════════════════════════════════════════

/// Read a capnp text field into an owned string. A missing field is the empty
/// string — the same equivalence the writers emit.
pub(crate) fn text_of(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

/// Split a URI into the contract's typed `T.Uri` fields.
pub(crate) fn write_uri(builder: &mut types_capnp::uri::Builder<'_>, uri: &str) {
    let (scheme, rest) = match uri.split_once(':') {
        Some((scheme, rest)) => (scheme, rest),
        None => ("", uri),
    };
    builder.set_scheme(scheme);
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let (authority, rest) = match rest.find(['/', '?', '#']) {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    builder.set_authority(authority);
    let (rest, fragment) = match rest.split_once('#') {
        Some((rest, fragment)) => (rest, fragment),
        None => (rest, ""),
    };
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, query),
        None => (rest, ""),
    };
    builder.set_path(path);
    builder.set_query(query);
    builder.set_fragment(fragment);
}

/// Read the contract's typed `T.Uri` back into one string.
pub(crate) fn read_uri(reader: &types_capnp::uri::Reader<'_>) -> String {
    let scheme = text_of(reader.get_scheme());
    let authority = text_of(reader.get_authority());
    let path = text_of(reader.get_path());
    let query = text_of(reader.get_query());
    let fragment = text_of(reader.get_fragment());
    let mut uri = String::new();
    if !scheme.is_empty() {
        uri.push_str(&scheme);
        uri.push(':');
    }
    if !authority.is_empty() {
        uri.push_str("//");
        uri.push_str(&authority);
    }
    uri.push_str(&path);
    if !query.is_empty() {
        uri.push('?');
        uri.push_str(&query);
    }
    if !fragment.is_empty() {
        uri.push('#');
        uri.push_str(&fragment);
    }
    uri
}

/// Days from 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub(crate) fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u32;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146097 + doe as i64 - 719468
}

/// The civil date for a day count (Howard Hinnant's algorithm).
pub(crate) fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// Parse a vCard date (`1990-01-01`, `1990-01`, `1990`, `19900101`) into
/// nanoseconds since the epoch (midnight UTC of that day), as a *signed*
/// count so a pre-epoch date keeps its sign. The time part of a date-time is
/// dropped, never guessed.
pub(crate) fn date_to_nanos(date: &str) -> Option<i64> {
    let (year, month, day) = parse_vcard_date(date)?;
    Some(days_from_civil(year, month, day) * 86_400_000_000_000)
}

/// Parse a vCard date into its calendar parts. The parts the card did not
/// state stay `0`.
pub(crate) fn parse_vcard_date(date: &str) -> Option<(i32, u32, u32)> {
    let text = date.trim();
    let text = text.split(['T', 't']).next().unwrap_or(text);
    let (year, rest) = match text.split_once('-') {
        Some((year, rest)) => (year, Some(rest)),
        None => (text, None),
    };
    let year: i32 = year.parse().ok()?;
    if !(1..=9999).contains(&year) {
        return None;
    }
    let rest = match rest {
        Some(rest) => rest,
        None => {
            // `1990` or `19900101`
            if text.len() == 8 && text.chars().all(|c| c.is_ascii_digit()) {
                let month: u32 = text[4..6].parse().ok()?;
                let day: u32 = text[6..8].parse().ok()?;
                return Some((year, month, day));
            }
            return Some((year, 0, 0));
        }
    };
    let (month, day) = match rest.split_once('-') {
        Some((month, day)) => (month, Some(day)),
        None => (rest, None),
    };
    let month: u32 = month.parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    let day = match day {
        Some(day) => {
            let day: u32 = day.parse().ok()?;
            if !(1..=31).contains(&day) {
                return None;
            }
            day
        }
        None => 0,
    };
    Some((year, month, day))
}

/// The date text for a timestamp (midnight UTC of that day).
pub(crate) fn nanos_to_date(nanos: u64) -> String {
    // A day is 86_400 seconds, so 86_400_000_000_000 nanoseconds. The count
    // is read back as the signed value the writer produced.
    let days = (nanos as i64).div_euclid(86_400_000_000_000);
    let (year, month, day) = civil_from_days(days);
    if month == 0 || day == 0 {
        format!("{year:04}")
    } else {
        format!("{year:04}-{month:02}-{day:02}")
    }
}
