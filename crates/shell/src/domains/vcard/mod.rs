//! `vcard` — the domain command module over the engine's own RFC 6350 parser
//! and formatter.
//!
//! The engine's vcard cell (`src/platforms/linux/vcard.rs`) is the engine's
//! own in-process parser and formatter: content lines with groups, the open
//! parameter list, line unfolding, the `\` escapes, structured values, base64
//! binary values, URI values and dates — read and written per RFC 6350. There
//! is no `libvcard` on this host and no shellout (law #9).
//!
//! ## What the shell shows is the engine's own mapping
//!
//! The contract's normative table (the one in `vcard.capnp`'s header) turns a
//! parsed card into a `Contact`. The shell renders the engine's OWNED types —
//! `VCard`, `VCardProperty`, `Contact`, `ImportResult`, `SyncResult` — so a
//! name on screen is a name the engine mapped, and the two rows the canonical
//! tree cannot carry (`ADR`'s PO-box and extended-address, and
//! `Contact.organization.title`, whose destination field does not exist) are
//! reported by the engine as warnings rather than dropped silently.
//!
//! ## Writes are gate-routed
//!
//! `import` changes the engine's store, and `export --out` writes a file on
//! the host — both carry a write intent and are accepted the way every other
//! mutating verb in this shell is accepted: `--force` one-shot, a confirmation
//! in the REPL.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::vcard::types::{
    Contact, DuplicateHandling, ExportRequest, FormatRequest, ImportRequest, OutputFormat as VCardOutput,
    PersonName, VCardVersion,
};
use smart_lib::domains::vcard::VcardBackend;

/// The engine seam this shell speaks to — the engine's vCard trait PLUS the
/// store's own read path, which the engine's cell exposes as an inherent
/// method (`contacts`).
pub trait VcardStore: VcardBackend {
    /// The contacts the store holds, in id order.
    fn contacts(&self) -> SmartResult<Vec<Contact>>;
}

/// The engine's Linux cell, taught the store's own read path: the engine
/// exposes it as an inherent method (`contacts`), and the shell reaches it
/// through its own trait so the command module keeps a `dyn` seam and a test
/// seam at the same time.
impl VcardStore for smart_lib::platforms::linux::vcard::LinuxVcardBackend {
    fn contacts(&self) -> SmartResult<Vec<Contact>> {
        smart_lib::platforms::linux::vcard::LinuxVcardBackend::contacts(self)
            .map_err(SmartError::from)
    }
}

/// The engine seam as the shell uses it: a trait object over the store trait.
pub type Backend = dyn VcardStore;

/// `smart vcard` — the engine's own vCard parser, formatter and store.
#[derive(Parser, Debug)]
pub struct VcardCmd {
    #[command(subcommand)]
    pub action: Option<VcardAction>,
}

#[derive(Subcommand, Debug)]
pub enum VcardAction {
    /// The contacts the store holds (the default view).
    Contacts,
    /// The vCard versions this engine can read and write.
    Versions,
    /// Parse a vCard file without importing it.
    Parse {
        /// The file to read (`-` reads stdin).
        path: String,
    },
    /// Import a vCard file into the store (a write — gate-routed).
    Import {
        /// The file to read (`-` reads stdin).
        path: String,
        /// What to do with a contact that already exists.
        #[arg(long, default_value = "update")]
        duplicate: String,
        /// Merge the imported card into the stored one.
        #[arg(long)]
        merge: bool,
        /// The version to parse with (auto detects it from the card).
        #[arg(long, default_value = "auto")]
        version: String,
    },
    /// Export the store's contacts as vCard data.
    Export {
        /// Only the contact with this id (repeatable).
        #[arg(long = "id")]
        ids: Vec<u64>,
        /// Export every contact.
        #[arg(long)]
        all: bool,
        /// The version to write.
        #[arg(long, default_value = "4.0")]
        version: String,
        /// The folding length in octets (0 folds nothing).
        #[arg(long, default_value_t = 75)]
        line_length: u16,
        /// Write the vCard to this file instead of the screen (a host write —
        /// gate-routed).
        #[arg(long)]
        out: Option<String>,
    },
    /// Format one stored contact as vCard data.
    Format {
        /// The contact's id.
        id: u64,
        /// The version to write.
        #[arg(long, default_value = "4.0")]
        version: String,
        /// The folding length in octets (0 folds nothing).
        #[arg(long, default_value_t = 75)]
        line_length: u16,
    },
    /// Build a contact from the command line and format it as vCard data (the
    /// card is formatted, never stored).
    New {
        /// The contact's display name.
        #[arg(long)]
        name: String,
        /// The given name.
        #[arg(long, default_value = "")]
        given: String,
        /// The family name.
        #[arg(long, default_value = "")]
        family: String,
        /// An email address (repeatable).
        #[arg(long = "email")]
        emails: Vec<String>,
        /// A phone number (repeatable).
        #[arg(long = "tel")]
        tels: Vec<String>,
        /// The organisation's name.
        #[arg(long, default_value = "")]
        org: String,
        /// The note.
        #[arg(long, default_value = "")]
        note: String,
        /// The version to write.
        #[arg(long, default_value = "4.0")]
        version: String,
    },
    /// A token that names the store's current state.
    SyncToken {
        /// Only the contacts with these ids (repeatable).
        #[arg(long = "id")]
        ids: Vec<u64>,
    },
    /// The cards that changed since a token.
    Sync {
        /// The token a previous `sync-token` handed back.
        token: String,
    },
    /// The backend's own capability tokens.
    Caps,
    /// The contract's availability matrix, exactly as the engine publishes it.
    Matrix,
}

/// Whether the caller accepted the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard {
    pub allowed: bool,
}

/// Evaluate the command's data half: the value the command produces, with no
/// rendering. This is what pipes consume.
pub fn evaluate(cmd: &VcardCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None | Some(VcardAction::Contacts) => Ok(render::contacts_value(backend)?),
        Some(VcardAction::Versions) => {
            let versions = backend.get_supported_versions().map_err(SmartError::from)?;
            Ok(render::versions_value(&versions))
        }
        Some(VcardAction::Parse { path }) => {
            let data = read_card(path)?;
            let cards = backend
                .parse_v_card(&data, VCardVersion::Auto)
                .map_err(SmartError::from)?;
            Ok(render::cards_value(&cards))
        }
        Some(VcardAction::Import {
            path,
            duplicate,
            merge,
            version,
        }) => {
            guard_host_write(guard, &format!("vcard import {path}"))?;
            let data = read_card(path)?;
            let request = ImportRequest {
                data,
                version: parse_version(version)?,
                group_id: 0,
                duplicate_handling: parse_duplicate(duplicate)?,
                merge: *merge,
                field_mapping: Vec::new(),
            };
            let result = backend.import_v_card(request).map_err(SmartError::from)?;
            Ok(render::import_value(&result))
        }
        Some(VcardAction::Export {
            ids,
            all,
            version,
            line_length,
            out,
        }) => {
            if let Some(path) = out {
                // Writing a file on the host is a host write: it carries the
                // gate like every other mutating verb.
                guard_host_write(guard, &format!("vcard export --out {path}"))?;
            }
            let request = ExportRequest {
                contact_ids: ids.clone(),
                all_contacts: *all || ids.is_empty(),
                version: parse_version(version)?,
                include_groups: Vec::new(),
                exclude_properties: Vec::new(),
                field_mapping: Vec::new(),
                line_length: *line_length,
                inline_photos: true,
                output_format: VCardOutput::Multiple,
            };
            let result = backend.export_v_card(request).map_err(SmartError::from)?;
            let text = String::from_utf8_lossy(&result.data).to_string();
            let written = match out {
                Some(path) => {
                    std::fs::write(path, &result.data)?;
                    Some(path.clone())
                }
                None => None,
            };
            Ok(render::export_value(&result, &text, written.as_deref()))
        }
        Some(VcardAction::Format {
            id,
            version,
            line_length,
        }) => {
            let contacts = backend.contacts()?;
            let Some(contact) = contacts.iter().find(|c| c.id == *id) else {
                return Err(SmartError::NotFound(format!(
                    "no contact with id {id} in the engine's store"
                )));
            };
            let request = FormatRequest {
                contact: contact.clone(),
                version: parse_version(version)?,
                line_length: *line_length,
                inline_photos: true,
                field_mapping: Vec::new(),
            };
            let data = backend.format_v_card(request).map_err(SmartError::from)?;
            let text = String::from_utf8_lossy(&data).to_string();
            Ok(render::formatted_value(*id, &text))
        }
        Some(VcardAction::New {
            name,
            given,
            family,
            emails,
            tels,
            org,
            note,
            version,
        }) => {
            let contact = Contact {
                id: 0,
                name: PersonName {
                    given: given.clone(),
                    family: family.clone(),
                    middle: String::new(),
                    prefix: String::new(),
                    suffix: String::new(),
                    display: name.clone(),
                },
                emails: emails
                    .iter()
                    .map(|email| smart_lib::domains::vcard::types::ContactEmail {
                        type_: "other".to_string(),
                        value: email.clone(),
                        primary: false,
                    })
                    .collect(),
                phones: tels
                    .iter()
                    .map(|tel| smart_lib::domains::vcard::types::ContactPhone {
                        type_: "other".to_string(),
                        value: tel.clone(),
                        raw: tel.clone(),
                        primary: false,
                    })
                    .collect(),
                organization: smart_lib::domains::vcard::types::ContactOrganization {
                    name: org.clone(),
                    department: String::new(),
                    role: String::new(),
                    industry: String::new(),
                },
                note: note.clone(),
                ..Contact::default()
            };
            let request = FormatRequest {
                contact,
                version: parse_version(version)?,
                line_length: 75,
                inline_photos: true,
                field_mapping: Vec::new(),
            };
            let data = backend.format_v_card(request).map_err(SmartError::from)?;
            let text = String::from_utf8_lossy(&data).to_string();
            Ok(render::formatted_value(0, &text))
        }
        Some(VcardAction::SyncToken { ids }) => {
            let token = backend.get_sync_token(ids).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("token".into(), Value::String(token)),
                ("ids".into(), Value::Number(ids.len() as f64)),
            ]))
        }
        Some(VcardAction::Sync { token }) => {
            let result = backend.sync_changes(token).map_err(SmartError::from)?;
            Ok(render::sync_value(&result))
        }
        Some(VcardAction::Caps) => Ok(render::caps_value(backend)),
        Some(VcardAction::Matrix) => Ok(render::matrix_value()),
    }
}

/// The card bytes a path names: a file, or stdin when the path is `-`.
fn read_card(path: &str) -> SmartResult<Vec<u8>> {
    if path == "-" {
        use std::io::Read;
        let mut data = Vec::new();
        std::io::stdin().read_to_end(&mut data)?;
        return Ok(data);
    }
    Ok(std::fs::read(path)?)
}

/// The version a command-line argument names. `auto` is the detection
/// request, never a version a card declares.
fn parse_version(text: &str) -> SmartResult<VCardVersion> {
    match text.trim() {
        "auto" => Ok(VCardVersion::Auto),
        "2.1" => Ok(VCardVersion::V21),
        "3.0" => Ok(VCardVersion::V30),
        "4.0" => Ok(VCardVersion::V40),
        other => Err(SmartError::invalid_argument(format!(
            "{other}: not a vCard version this engine speaks (2.1, 3.0, 4.0, or auto)"
        ))),
    }
}

/// The duplicate handling a command-line argument names.
fn parse_duplicate(text: &str) -> SmartResult<DuplicateHandling> {
    match text.trim().to_lowercase().as_str() {
        "skip" => Ok(DuplicateHandling::Skip),
        "update" => Ok(DuplicateHandling::Update),
        "create" => Ok(DuplicateHandling::Create),
        "merge" => Ok(DuplicateHandling::Merge),
        other => Err(SmartError::invalid_argument(format!(
            "{other}: not a duplicate handling (skip, update, create, merge)"
        ))),
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(
    cmd: &VcardCmd,
    format: &OutputFormat,
    backend: &Backend,
    guard: WriteGuard,
) -> SmartResult<Value> {
    let value = evaluate(cmd, backend, guard)?;
    if *format != OutputFormat::Raw {
        println!("{}", render::render_action(cmd, &value, *format));
    }
    Ok(value)
}

/// The write this command performs, when it performs one.
pub fn write_intent(cmd: &VcardCmd) -> Option<(String, String)> {
    use VcardAction as V;
    Some(match &cmd.action {
        Some(V::Import { path, .. }) => ("import vcard file".into(), path.clone()),
        Some(V::Export { out: Some(path), .. }) => ("write vcard file".into(), path.clone()),
        _ => return None,
    })
}

pub mod render;

/// The host-write gate: a mutating verb proceeds only when the caller accepted
/// the write.
fn guard_host_write(guard: WriteGuard, verb: &str) -> SmartResult<()> {
    if guard.allowed {
        return Ok(());
    }
    Err(SmartError::not_supported(format!(
        "refusing to run `{verb}`: it CHANGES the engine's store or the host's files. The \
         interactive `(y/N)` prompt asks with the target's context beside it; a piped `y` line \
         answers it, and `--force` accepts the write explicitly."
    )))
}

/// The vcard selector tree, rendered from the engine's own answers.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    Ok(Value::Object(vec![
        ("contacts".into(), render::contacts_value(backend)?),
        ("versions".into(), render::versions_value(
            &backend.get_supported_versions().map_err(SmartError::from)?,
        )),
        ("caps".into(), render::caps_value(backend)),
    ]))
}

/// Re-export for the shell and main: the clap shape.
pub use self::{VcardAction as Action, VcardCmd as Cmd};
