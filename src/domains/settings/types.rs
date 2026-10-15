//! Owned value types the **settings** domain speaks (the types a caller
//! actually touches), plus their conversions to and from the frozen
//! `settings.capnp`.
//!
//! Law #7: the capnp state structs are generated; this module is the
//! *hand-written* mirror used at the domain's public/backend seam.
//!
//! ## The value model is recursive, and its zero member is not the answer
//!
//! `SettingValue`'s zero member is `string`, which is exactly why the contract
//! changed `get` to return a union: "a consumer reading `ui.accent` that was
//! never set painted an empty accent instead of applying the default, and every
//! backend invented a different sentinel". [`SettingResult`] therefore mirrors
//! that union exactly — `Value`, `NotSet`, `Error`, `Unknown` — so a caller
//! cannot forget the difference between "set to the empty string" and "never
//! set".
//!
//! ## Where a value comes from on this host
//!
//! The Linux backend speaks the host's own settings engine (glib's `GSettings`,
//! which is what `gsettings` is a CLI over) through the maintained `gio` crate:
//! the schema and its defaults come from the host's own compiled schema store,
//! and a per-user override comes from the host's own dconf database — the same
//! bytes the host's own desktop writes. There is no `gsettings` fork.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::values::Timestamp;
use crate::schema::{settings_capnp as capnp, types_capnp};

/// The contract's `T.StringMap` as an ordered pair list, in the peer's order.
pub type Metadata = Vec<(String, String)>;

/// Pagination, mirroring `T.Pagination` (`offset`, `limit`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    pub offset: u32,
    pub limit: u32,
}

impl Pagination {
    /// Effective limit under the contract rule: 0 means "one".
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 {
            1
        } else {
            self.limit
        }
    }
}

/// A one-page window over an unbounded list, with the stable `total`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
}

/// The contract's page window over an already-materialized list.
pub(crate) fn page_window<T: Clone>(items: &[T], page: Pagination) -> Page<T> {
    let total = items.len() as u64;
    let start = page.offset as usize;
    let end = start
        .saturating_add(page.effective_limit() as usize)
        .min(items.len());
    let items = if start >= items.len() {
        Vec::new()
    } else {
        items[start..end].to_vec()
    };
    Page { items, total }
}

/// Read a `Text` field into an owned string (absent/invalid UTF-8 is empty).
pub(crate) fn text_field(reader: ::capnp::Result<::capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

/// The contract's own conformance category tokens — conventions, not contract.
/// An unrecognised value MUST be ignored, never rejected.
pub const CONFORMANCE_CATEGORIES: &[&str] = &[
    "security",
    "display",
    "accessibility",
    "network",
    "system",
    "user",
    "audio",
    "privacy",
    "developer",
];

/// One setting value (`SettingValue`): the recursive value model, with the
/// contract's own `unknown` arm for a type this schema was frozen before.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingValue {
    String(String),
    Integer(i64),
    Real(f64),
    Boolean(bool),
    Data(Vec<u8>),
    List(Vec<SettingValue>),
    Dict(Vec<(String, SettingValue)>),
    Unknown,
}

impl Default for SettingValue {
    /// A value nobody described is the empty string, which is what the union's
    /// zero member is — and the reason a caller must read [`SettingResult`]
    /// rather than a bare value, since `NotSet` is its own arm there.
    fn default() -> Self {
        SettingValue::String(String::new())
    }
}

impl SettingValue {
    /// The open token a CLI renders.
    pub const fn type_token(&self) -> &'static str {
        match self {
            SettingValue::String(_) => "string",
            SettingValue::Integer(_) => "integer",
            SettingValue::Real(_) => "real",
            SettingValue::Boolean(_) => "boolean",
            SettingValue::Data(_) => "data",
            SettingValue::List(_) => "list",
            SettingValue::Dict(_) => "dict",
            SettingValue::Unknown => "unknown",
        }
    }

    /// Whether this is the union's zero member (the empty string), which is the
    /// spelling a consumer must be able to tell apart from `NotSet`.
    pub fn is_empty_string(&self) -> bool {
        matches!(self, SettingValue::String(text) if text.is_empty())
    }

    /// Write into the generated builder.
    pub fn write_to<'a>(&self, mut builder: capnp::setting_value::Builder<'a>) {
        match self {
            SettingValue::String(text) => {
                builder.set_string(text);
            }
            SettingValue::Integer(value) => {
                builder.set_integer(*value);
            }
            SettingValue::Real(value) => {
                builder.set_real(*value);
            }
            SettingValue::Boolean(value) => {
                builder.set_boolean(*value);
            }
            SettingValue::Data(bytes) => {
                // `T.DataHandle`'s own union: the bytes ride the inline arm,
                // which is the payload shape this schema carries.
                let mut handle = builder.reborrow().init_data();
                handle.set_inline(&bytes);
            }
            SettingValue::List(items) => {
                let mut list = builder.reborrow().init_list(items.len() as u32);
                for (index, item) in items.iter().enumerate() {
                    item.write_to(list.reborrow().get(index as u32));
                }
            }
            SettingValue::Dict(entries) => {
                let mut map = builder.reborrow().init_dict();
                let mut list = map.reborrow().init_entries(entries.len() as u32);
                for (index, (key, value)) in entries.iter().enumerate() {
                    let mut entry = list.reborrow().get(index as u32);
                    entry.set_key(key);
                    value.write_to(entry.init_value());
                }
            }
            SettingValue::Unknown => {
                // The contract's `unknown` arm is a `T.UnknownEvent`: an opaque
                // payload this backend never fabricates, so it stays default.
                let _ = builder;
            }
        }
    }

    /// Read from the generated reader.
    pub fn from_reader(reader: &capnp::setting_value::Reader<'_>) -> Self {
        use capnp::setting_value::Which;
        match reader.which() {
            Ok(Which::String(text)) => SettingValue::String(text_field(text)),
            Ok(Which::Integer(value)) => SettingValue::Integer(value),
            Ok(Which::Real(value)) => SettingValue::Real(value),
            Ok(Which::Boolean(value)) => SettingValue::Boolean(value),
            Ok(Which::Data(data)) => {
                // `T.DataHandle` is a union, so the read takes the inline arm
                // the writers emit and answers empty for a buffer reference.
                use types_capnp::data_handle::WhichReader;
                SettingValue::Data(
                    data.ok()
                        .and_then(|handle| match handle.which() {
                            Ok(WhichReader::Inline(inline)) => Some(inline.unwrap_or_default().to_vec()),
                            _ => None,
                        })
                        .unwrap_or_default(),
                )
            }
            Ok(Which::List(list)) => SettingValue::List(
                list.map(|items| {
                    items
                        .iter()
                        .map(|item| SettingValue::from_reader(&item))
                        .collect()
                })
                .unwrap_or_default(),
            ),
            Ok(Which::Dict(map)) => SettingValue::Dict(
                map.ok()
                    .and_then(|entries| entries.get_entries().ok())
                    .map(|entries| {
                        entries
                            .iter()
                            .map(|entry| {
                                (
                                    text_field(entry.get_key()),
                                    SettingValue::from_reader(&entry.get_value().unwrap()),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            Ok(Which::Unknown(_)) => SettingValue::Unknown,
            Err(_) => SettingValue::Unknown,
        }
    }
}

/// The `get` result: the union the contract carries so a missing key is an
/// ANSWER and not a sentinel.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingResult {
    /// The key's own value.
    Value(SettingValue),
    /// The key is not set — an answer, not a fault.
    NotSet,
    /// The host's own error.
    Error(crate::core::Error),
    /// The contract's unknown arm.
    Unknown,
}

/// One entry of a multi-entry read or write (`SettingMapEntry`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingMapEntry {
    pub key: String,
    pub value: SettingValue,
}

/// A multi-entry read/write (`SettingMap`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingMap {
    pub entries: Vec<SettingMapEntry>,
}

/// One change (`SettingChange`): the contract's own audit shape.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingChange {
    pub domain: String,
    pub key: String,
    pub value: SettingValue,
    pub timestamp: Timestamp,
    /// An opaque actor token, backend-decided.
    pub actor_id: String,
    /// The store version after this change.
    pub version: u64,
}

/// One schema key, as the host's own published table describes it.
///
/// This is the surface that makes a read honest: the type, the default and the
/// range are the host's own declarations, so a caller can tell "the host says
/// this defaults to X" from "somebody guessed X".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemaKey {
    /// The key's own name (the last path component a caller passes).
    pub name: String,
    /// The host's own GVariant type string (`s`, `b`, `i`, `as`, …).
    pub variant_type: String,
    /// The host's own default value, printed in GVariant's own text syntax.
    pub default_value: Option<String>,
    /// The host's own range as its `(sv)` spelling, when the schema declares
    /// one.
    pub range: Option<String>,
    /// The host's own summary line.
    pub summary: Option<String>,
    /// The host's own description.
    pub description: Option<String>,
}

/// One schema, as the host's own published table describes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemaInfo {
    /// The schema id a caller passes as `domain`.
    pub id: String,
    /// The host's own object path, when the schema is not relocatable.
    pub path: Option<String>,
    /// Whether the schema is relocatable (has no fixed object path).
    pub relocatable: bool,
    /// The schema's keys, in the host's own order.
    pub keys: Vec<SchemaKey>,
}

impl SchemaInfo {
    /// Does this schema carry the key a caller named?
    pub fn has_key(&self, key: &str) -> bool {
        self.keys.iter().any(|candidate| candidate.name == key)
    }

    /// The host's own declaration for one key.
    pub fn key(&self, key: &str) -> Option<&SchemaKey> {
        self.keys.iter().find(|candidate| candidate.name == key)
    }
}

impl SchemaKey {
    /// The contract's own value model this key's type maps to, when it maps at
    /// all. A key whose type is not one of the contract's seven forms is
    /// reported as `Unknown` rather than coerced into the wrong arm.
    pub fn contract_value(&self) -> Option<&'static str> {
        match self.variant_type.as_str() {
            "s" | "ms" => Some("string"),
            "b" | "mb" => Some("boolean"),
            "i" | "u" | "x" | "q" | "h" | "y" | "n" => Some("integer"),
            "d" => Some("real"),
            "as" | "ao" | "at" => Some("list"),
            "a{ss}" => Some("dict"),
            "ay" => Some("data"),
            _ => None,
        }
    }
}

impl SettingResult {
    /// The value, or `None` when the union's other arm is the answer.
    pub fn value(&self) -> Option<&SettingValue> {
        match self {
            SettingResult::Value(value) => Some(value),
            _ => None,
        }
    }
}
