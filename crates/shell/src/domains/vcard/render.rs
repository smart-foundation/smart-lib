//! The vcard domain's rendering, in the shell's aesthetic: box-drawing,
//! `◉/○`, aligned tables. Every renderer reads the engine's OWNED types
//! (`Contact`, `VCard`, `VCardProperty`, `ImportResult`, `SyncResult`) — the
//! rows are the engine's own mapping of the cards the host published.

use crate::color::ColorExt;
use crate::selector::Value;
use crate::widgets::{truncate_to, Table};
use crate::OutputFormat;

use smart_lib::domains::vcard::types::{
    Contact, ExportResult, ImportResult, SyncResult, VCard, VCardVersion,
};
use smart_lib::domains::vcard::VcardBackend;

use super::VcardStore;

/// The widest the NAME column may grow before the cut is marked.
const NAME_COLUMN: usize = 30;
/// The widest the DETAIL column may grow before the cut is marked.
const DETAIL_COLUMN: usize = 44;
/// The widest a property's value column may grow before the cut is marked.
const VALUE_COLUMN: usize = 56;

/// The contacts the store holds, as the engine's own contacts.
pub fn contacts_value(backend: &dyn VcardStore) -> Result<Value, crate::error::SmartError> {
    let contacts = backend.contacts()?;
    Ok(Value::List(contacts.iter().map(contact_value).collect()))
}

/// One contact, as the engine's own `Contact`.
pub fn contact_value(contact: &Contact) -> Value {
    let emails = contact
        .emails
        .iter()
        .map(|e| e.value.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let phones = contact
        .phones
        .iter()
        .map(|p| p.value.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let addresses = contact
        .addresses
        .iter()
        .map(|a| {
            let mut parts = vec![a.street.clone(), a.city.clone(), a.region.clone()];
            parts.retain(|p| !p.is_empty());
            parts.join(", ")
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    Value::Object(vec![
        ("id".into(), Value::Number(contact.id as f64)),
        ("name".into(), Value::String(contact.name.display.clone())),
        ("given".into(), Value::String(contact.name.given.clone())),
        ("family".into(), Value::String(contact.name.family.clone())),
        ("emails".into(), Value::String(emails)),
        ("phones".into(), Value::String(phones)),
        ("addresses".into(), Value::String(addresses)),
        ("organization".into(), Value::String(contact.organization.name.clone())),
        ("note".into(), Value::String(contact.note.clone())),
        ("tags".into(), Value::String(contact.tags.join(", "))),
        ("photo".into(), Value::Bool(contact.photo.is_some())),
        (
            "birthday".into(),
            match contact.birthday {
                Some(date) if !date.is_empty() => Value::String(format!(
                    "{:04}-{:02}-{:02}",
                    date.year, date.month, date.day
                )),
                _ => Value::Null,
            },
        ),
        (
            "metadata".into(),
            Value::List(
                contact
                    .metadata
                    .iter()
                    .map(|(k, v)| {
                        Value::Object(vec![
                            ("key".into(), Value::String(k.clone())),
                            ("value".into(), Value::String(v.clone())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// The versions the engine can read and write.
pub fn versions_value(versions: &[VCardVersion]) -> Value {
    Value::List(
        versions
            .iter()
            .map(|v| Value::Object(vec![("version".into(), Value::String(v.as_str().to_string()))]))
            .collect(),
    )
}

/// The parsed cards, with their properties as the engine read them.
pub fn cards_value(cards: &[VCard]) -> Value {
    Value::List(
        cards
            .iter()
            .map(|card| {
                Value::Object(vec![
                    ("version".into(), Value::String(card.version.as_str().to_string())),
                    ("properties".into(), Value::Number(card.properties.len() as f64)),
                    (
                        "lines".into(),
                        Value::List(
                            card.properties
                                .iter()
                                .map(|property| {
                                    let parameters = property
                                        .parameters
                                        .iter()
                                        .map(|p| format!("{}={}", p.name, p.value))
                                        .collect::<Vec<_>>()
                                        .join(";");
                                    let group = if property.group.is_empty() {
                                        String::new()
                                    } else {
                                        format!("{}.", property.group)
                                    };
                                    let values = property
                                        .values
                                        .iter()
                                        .map(|value| match value {
                                            smart_lib::domains::vcard::types::VCardValue::Text(text) => {
                                                text.clone()
                                            }
                                            smart_lib::domains::vcard::types::VCardValue::Structured(
                                                parts,
                                            ) => parts.join(";"),
                                            smart_lib::domains::vcard::types::VCardValue::Uri(uri) => {
                                                uri.clone()
                                            }
                                            smart_lib::domains::vcard::types::VCardValue::Date(date) => {
                                                date.clone()
                                            }
                                            smart_lib::domains::vcard::types::VCardValue::Boolean(b) => {
                                                b.to_string()
                                            }
                                            smart_lib::domains::vcard::types::VCardValue::Integer(i) => {
                                                i.to_string()
                                            }
                                            smart_lib::domains::vcard::types::VCardValue::Float(f) => {
                                                f.to_string()
                                            }
                                            smart_lib::domains::vcard::types::VCardValue::Binary(bytes) => {
                                                format!("<{} byte(s)>", bytes.len())
                                            }
                                            smart_lib::domains::vcard::types::VCardValue::Custom(bytes) => {
                                                format!("<{} byte(s)>", bytes.len())
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join(",");
                                    let head = if parameters.is_empty() {
                                        format!("{group}{}", property.name)
                                    } else {
                                        format!("{group}{};{parameters}", property.name)
                                    };
                                    Value::Object(vec![
                                        ("line".into(), Value::String(head.clone())),
                                        ("value".into(), Value::String(values)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect(),
    )
}

/// An import's own report: the counts, the per-contact rows and the engine's
/// warnings, verbatim.
pub fn import_value(result: &ImportResult) -> Value {
    Value::Object(vec![
        ("processed".into(), Value::Number(result.total_processed as f64)),
        ("imported".into(), Value::Number(result.imported as f64)),
        ("skipped".into(), Value::Number(result.skipped as f64)),
        ("failed".into(), Value::Number(result.failed as f64)),
        (
            "contacts".into(),
            Value::List(
                result
                    .contacts
                    .iter()
                    .map(|contact| {
                        Value::Object(vec![
                            ("id".into(), Value::Number(contact.contact_id as f64)),
                            ("uid".into(), Value::String(contact.vcard_uid.clone())),
                            ("created".into(), Value::Bool(contact.created)),
                            (
                                "warnings".into(),
                                Value::List(
                                    contact
                                        .warnings
                                        .iter()
                                        .map(|w| Value::String(w.clone()))
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "warnings".into(),
            Value::List(result.warnings.iter().map(|w| Value::String(w.clone())).collect()),
        ),
        (
            "errors".into(),
            Value::List(
                result
                    .errors
                    .iter()
                    .map(|error| {
                        Value::Object(vec![
                            ("uid".into(), Value::String(error.vcard_uid.clone())),
                            ("line".into(), Value::Number(error.line as f64)),
                            ("reason".into(), Value::String(error.reason.clone())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// An export's own report, with the vCard text it produced.
pub fn export_value(result: &ExportResult, text: &str, written: Option<&str>) -> Value {
    Value::Object(vec![
        ("exported".into(), Value::Number(result.exported as f64)),
        ("size".into(), Value::Number(result.data.len() as f64)),
        ("card".into(), Value::String(text.to_string())),
        (
            "written_to".into(),
            match written {
                Some(path) => Value::String(path.to_string()),
                None => Value::Null,
            },
        ),
        (
            "warnings".into(),
            Value::List(result.warnings.iter().map(|w| Value::String(w.clone())).collect()),
        ),
        (
            "errors".into(),
            Value::List(result.errors.iter().map(|e| Value::String(e.clone())).collect()),
        ),
    ])
}

/// One formatted card.
pub fn formatted_value(id: u64, text: &str) -> Value {
    Value::Object(vec![
        ("id".into(), Value::Number(id as f64)),
        ("card".into(), Value::String(text.to_string())),
    ])
}

/// A sync's own report: the new token, the changed cards and the deletions.
pub fn sync_value(result: &SyncResult) -> Value {
    Value::Object(vec![
        ("token".into(), Value::String(result.new_sync_token.clone())),
        ("changed".into(), Value::Number(result.changed.len() as f64)),
        ("deleted".into(), Value::Number(result.deleted.len() as f64)),
        ("has_more".into(), Value::Bool(result.has_more)),
        ("conflicts".into(), Value::Number(result.conflicts.len() as f64)),
        (
            "cards".into(),
            Value::List(
                result
                    .changed
                    .iter()
                    .map(|card| {
                        Value::Object(vec![
                            ("version".into(), Value::String(card.version.as_str().to_string())),
                            (
                                "name".into(),
                                Value::String(
                                    card.first_text("FN").unwrap_or_default().to_string(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "deleted_ids".into(),
            Value::List(result.deleted.iter().map(|id| Value::Number(*id as f64)).collect()),
        ),
    ])
}

/// The backend's own capability tokens.
pub fn caps_value(backend: &dyn VcardBackend) -> Value {
    Value::List(
        backend
            .vcard_caps()
            .into_iter()
            .map(|cap| Value::String(cap.to_string()))
            .collect(),
    )
}

/// The contract's availability matrix, exactly as the engine publishes it.
pub fn matrix_value() -> Value {
    use smart_lib::domains::vcard::{AvailStatus, VCARD_AVAILABILITY, VCARD_SCHEMA_ID};
    let rows = VCARD_AVAILABILITY
        .iter()
        .map(|row| {
            let status = match row.status {
                AvailStatus::Real => "real",
                AvailStatus::Degraded => "degraded",
                AvailStatus::Unavailable => "unavailable",
            };
            Value::Object(vec![
                ("ordinal".into(), Value::Number(row.ordinal as f64)),
                ("method".into(), Value::String(row.name.to_string())),
                ("status".into(), Value::String(status.to_string())),
                (
                    "reason_class".into(),
                    match row.reason_class {
                        Some(class) => Value::String(class.as_str().to_string()),
                        None => Value::Null,
                    },
                ),
                ("reason".into(), Value::String(row.reason.to_string())),
                (
                    "l3_test".into(),
                    match row.l3_test {
                        Some(test) => Value::String(test.to_string()),
                        None => Value::Null,
                    },
                ),
            ])
        })
        .collect();
    Value::Object(vec![
        ("schema_id".into(), Value::String(VCARD_SCHEMA_ID.to_string())),
        ("rows".into(), Value::List(rows)),
    ])
}

/// Render one command's value in the requested format.
pub fn render_action(cmd: &super::VcardCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &super::VcardCmd, value: &Value) -> String {
    use super::VcardAction as V;
    match &cmd.action {
        None | Some(V::Contacts) => render_contacts(value),
        Some(V::Versions) => render_versions(value),
        Some(V::Parse { .. }) => render_cards(value),
        Some(V::Import { .. }) => render_import(value),
        Some(V::Export { .. }) => render_export(value),
        Some(V::Format { .. }) | Some(V::New { .. }) => render_card(value),
        Some(V::SyncToken { .. }) => format!(
            "  {} {} — {}",
            "📇".blue().bold(),
            "Sync token".bold(),
            value.field_str("token")
        ),
        Some(V::Sync { .. }) => render_sync(value),
        Some(V::Caps) => format!(
            "  {} {} — {}",
            "📇".blue().bold(),
            "Capabilities".blue().bold(),
            value.display()
        ),
        Some(V::Matrix) => render_matrix(value),
    }
}

fn render_contacts(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "📇".blue().bold(), "vCards".blue().bold()));
    match value {
        Value::List(items) if !items.is_empty() => {
            out.push_str(&format!("  ├─ {} contact(s) in the engine's store\n", items.len()));
            let mut table = Table::new(vec![
                "ID".into(),
                "NAME".into(),
                "EMAILS".into(),
                "PHONES".into(),
                "ORG".into(),
            ])
            .cap(1, NAME_COLUMN)
            .cap(2, DETAIL_COLUMN)
            .cap(3, DETAIL_COLUMN);
            for item in items {
                table = table.row(vec![
                    item.field("id").display(),
                    item.field_str("name"),
                    item.field_str("emails"),
                    item.field_str("phones"),
                    item.field_str("organization"),
                ]);
            }
            out.push_str(&table.render());
            out.push('\n');
        }
        _ => {
            out.push_str(&format!(
                "  └─ {} — the engine's store holds no contact yet (an honest empty;\n",
                "○".dimmed()
            ));
            out.push_str("      `vcard import` puts one there)\n");
        }
    }
    out
}

fn render_versions(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {}\n",
        "📇".blue().bold(),
        "Supported versions".blue().bold()
    ));
    if let Value::List(items) = value {
        for item in items {
            out.push_str(&format!("  ├─ {}\n", item.field_str("version")));
        }
    }
    out
}

fn render_cards(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {}\n",
        "📇".blue().bold(),
        "Parsed cards".blue().bold()
    ));
    if let Value::List(cards) = value {
        for card in cards {
            out.push_str(&format!(
                "  ├─ vCard {}\n",
                card.field_str("version")
            ));
            if let Value::List(lines) = card.field("lines") {
                let mut table = Table::new(vec!["LINE".into(), "VALUE".into()]).cap(1, VALUE_COLUMN);
                for line in lines {
                    table = table.row(vec![line.field_str("line"), line.field_str("value")]);
                }
                out.push_str(&table.render());
                out.push('\n');
            }
        }
    }
    out
}

fn render_import(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "📇".blue().bold(), "Import".blue().bold()));
    out.push_str(&format!(
        "  ├─ Processed  {}\n",
        value.field("processed").display()
    ));
    out.push_str(&format!(
        "  ├─ Imported   {}\n",
        value.field("imported").display()
    ));
    out.push_str(&format!(
        "  ├─ Skipped    {}\n",
        value.field("skipped").display()
    ));
    out.push_str(&format!(
        "  ├─ Failed     {}\n",
        value.field("failed").display()
    ));
    if let Value::List(contacts) = value.field("contacts") {
        out.push_str(&format!("  ├─ Contacts   {}\n", contacts.len()));
        let mut table = Table::new(vec!["ID".into(), "UID".into(), "CREATED".into()]).cap(1, NAME_COLUMN);
        for contact in contacts {
            table = table.row(vec![
                contact.field("id").display(),
                contact.field_str("uid"),
                if contact.field_bool("created") {
                    "◉ yes".to_string()
                } else {
                    "○ no".to_string()
                },
            ]);
        }
        out.push_str(&table.render());
        out.push('\n');
    }
    if let Value::List(warnings) = value.field("warnings") {
        if !warnings.is_empty() {
            out.push_str(&format!("  ├─ Warnings  {}\n", warnings.len()));
            for warning in warnings {
                out.push_str(&format!(
                    "     ├─ {}\n",
                    truncate_to(&warning.display(), VALUE_COLUMN)
                ));
            }
        }
    }
    if let Value::List(errors) = value.field("errors") {
        if !errors.is_empty() {
            out.push_str(&format!("  ├─ Errors    {}\n", errors.len()));
            for error in errors {
                out.push_str(&format!("     ├─ {}\n", error.field_str("reason")));
            }
        }
    }
    out
}

fn render_export(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "📇".blue().bold(), "Export".blue().bold()));
    out.push_str(&format!(
        "  ├─ Exported  {}\n",
        value.field("exported").display()
    ));
    out.push_str(&format!(
        "  ├─ Size      {} byte(s)\n",
        value.field("size").display()
    ));
    match value.field("written_to") {
        Value::String(path) => out.push_str(&format!("  ├─ Written   {path}\n")),
        _ => {}
    }
    out.push_str("  └─ Card\n");
    for line in value.field_str("card").lines() {
        out.push_str(&format!("     {line}\n"));
    }
    out
}

fn render_card(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — {}\n",
        "📇".blue().bold(),
        "Formatted card".bold(),
        value.field_str("id").dimmed()
    ));
    for line in value.field_str("card").lines() {
        out.push_str(&format!("     {line}\n"));
    }
    out
}

fn render_sync(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "📇".blue().bold(), "Sync".blue().bold()));
    out.push_str(&format!("  ├─ Token     {}\n", value.field_str("token")));
    out.push_str(&format!(
        "  ├─ Changed   {}\n",
        value.field("changed").display()
    ));
    out.push_str(&format!(
        "  ├─ Deleted   {}\n",
        value.field("deleted").display()
    ));
    out.push_str(&format!(
        "  ├─ Conflicts {}\n",
        value.field("conflicts").display()
    ));
    out.push_str(&format!(
        "  └─ Has more  {}\n",
        if value.field_bool("has_more") { "yes" } else { "no" }
    ));
    out
}

fn render_matrix(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — schema {}\n",
        "📇".blue().bold(),
        "Availability".blue().bold(),
        value.field_str("schema_id")
    ));
    out.push_str("  the contract's surface as the ENGINE's own matrix publishes it\n");
    if let Value::List(rows) = value.field("rows") {
        let mut table = Table::new(vec![
            "#".into(),
            "METHOD".into(),
            "STATUS".into(),
            "REASON".into(),
        ])
        .cap(3, 60);
        for row in rows {
            let status = row.field_str("status");
            let glyph = match status.as_str() {
                "real" => "◉".green().to_string(),
                "degraded" => "◐".yellow().to_string(),
                _ => "○".dimmed().to_string(),
            };
            // A REAL row carries no reason — it carries the L3 test that
            // PROVES it, and that proof is what the column shows.
            let reason = if row.field_str("reason").is_empty() {
                let proof = row.field_str("l3_test");
                if proof.is_empty() || proof == "null" {
                    "—".to_string()
                } else {
                    proof
                }
            } else {
                row.field_str("reason")
            };
            table = table.row(vec![
                row.field("ordinal").display(),
                row.field_str("method"),
                format!("{glyph} {status}"),
                reason,
            ]);
        }
        out.push_str(&table.render());
        out.push('\n');
    }
    out
}
