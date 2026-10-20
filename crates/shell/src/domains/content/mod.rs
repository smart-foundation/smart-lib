//! `content` — the domain command module over the engine's own content store.
//!
//! The engine's content cell (`src/platforms/linux/content.rs`) is the
//! engine's own in-process store: real items with real ids, versions,
//! timestamps, payloads and tags, and the contract's own optimistic
//! concurrency check — a write that presents a stale version is REFUSED with
//! a conflict, never a silent overwrite. The shell renders from the engine's
//! OWNED types (`ContentItem`, `ContentFilter`).
//!
//! ## The store's lifetime is the engine's own
//!
//! It lives in the process that built it, so the shell's session keeps ONE
//! backend per process: `publish → get` composes across the lines of one
//! session, and a one-shot invocation is one session.
//!
//! ## Writes are gate-routed
//!
//! `publish`, `publish-many`, `update`, `unpublish`, `delete`, `trash`,
//! `restore` and `empty-trash` change the store, so they carry a write intent
//! and are accepted the way every other mutating verb in this shell is
//! accepted: `--force` one-shot, a confirmation in the REPL.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::content::types::{ContentFilter, ContentItem, Pagination};
use smart_lib::domains::content::ContentBackend;

/// The engine seam this shell speaks to.
pub type Backend = dyn ContentBackend;

/// The page window a command asks for.
fn page(offset: u32, limit: u32) -> Pagination {
    Pagination {
        offset,
        limit: limit.max(1),
    }
}

/// `smart content` — the engine's own content store.
#[derive(Parser, Debug)]
pub struct ContentCmd {
    #[command(subcommand)]
    pub action: Option<ContentAction>,
}

#[derive(Subcommand, Debug)]
pub enum ContentAction {
    /// The items the store holds (the default view).
    List {
        /// A substring of the title, the summary or the payload.
        #[arg(long, default_value = "")]
        query: String,
        /// Only the items whose MIME type starts with this prefix.
        #[arg(long, default_value = "")]
        mime: String,
        /// Only the items carrying this tag (repeatable).
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Include the trashed items.
        #[arg(long)]
        include_trashed: bool,
        /// The first row to return.
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// How many rows to return.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// One item, by the id the store assigned.
    Get {
        /// The item's id.
        id: u64,
    },
    /// Publish one item (a write — gate-routed).
    Publish {
        /// The title.
        title: String,
        /// The summary.
        #[arg(long, default_value = "")]
        summary: String,
        /// The URI, when the item is a reference rather than a payload.
        #[arg(long, default_value = "")]
        uri: String,
        /// The MIME type.
        #[arg(long, default_value = "text/plain")]
        mime: String,
        /// The inline payload (text; the engine stores its UTF-8 bytes).
        #[arg(long, default_value = "")]
        body: String,
        /// A tag (repeatable).
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// Publish several items at once — one per `--title`, each with the
    /// shared `--summary` / `--body` / `--mime` / `--tag` fields (a write —
    /// gate-routed).
    PublishMany {
        /// The titles — one item per title.
        #[arg(required = true)]
        titles: Vec<String>,
        /// The summary every item carries.
        #[arg(long, default_value = "")]
        summary: String,
        /// The MIME type every item carries.
        #[arg(long, default_value = "text/plain")]
        mime: String,
        /// The inline payload every item carries.
        #[arg(long, default_value = "")]
        body: String,
        /// A tag every item carries (repeatable).
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// Update one item, presenting the version the caller last saw (a write —
    /// gate-routed).
    Update {
        /// The item's id.
        id: u64,
        /// The version the write presents — the store refuses a stale one.
        #[arg(long, default_value_t = 1)]
        expected_version: u64,
        /// The new title.
        #[arg(long)]
        title: Option<String>,
        /// The new summary.
        #[arg(long)]
        summary: Option<String>,
        /// The new URI.
        #[arg(long)]
        uri: Option<String>,
        /// The new MIME type.
        #[arg(long)]
        mime: Option<String>,
        /// The new inline payload.
        #[arg(long)]
        body: Option<String>,
        /// A tag (repeatable — the item's tags are replaced by these).
        #[arg(long = "tag")]
        tags: Option<Vec<String>>,
    },
    /// Unpublish an item (a write — gate-routed).
    Unpublish {
        /// The item's id.
        id: u64,
    },
    /// Delete an item outright (a write — gate-routed).
    Delete {
        /// The item's id.
        id: u64,
    },
    /// Move an item to the trash (a write — gate-routed).
    Trash {
        /// The item's id.
        id: u64,
    },
    /// Bring an item back from the trash (a write — gate-routed).
    Restore {
        /// The item's id.
        id: u64,
    },
    /// Empty the trash (a write — gate-routed).
    EmptyTrash,
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
pub fn evaluate(cmd: &ContentCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None => Ok(render::list_value(
            backend,
            &ContentFilter::default(),
            page(0, 50),
        )?),
        Some(ContentAction::List {
            query,
            mime,
            tags,
            include_trashed,
            offset,
            limit,
        }) => {
            let filter = ContentFilter {
                query: query.clone(),
                mime_type_prefix: mime.clone(),
                tags: tags.clone(),
                include_trashed: *include_trashed,
            };
            Ok(render::list_value(backend, &filter, page(*offset, *limit))?)
        }
        Some(ContentAction::Get { id }) => {
            let item = backend.get(*id).map_err(|e| {
                match SmartError::from(e) {
                    SmartError::NotFound(_) => SmartError::NotFound(format!(
                        "the engine's store holds no item with id {id}"
                    )),
                    other => other,
                }
            })?;
            Ok(render::item_value(&item))
        }
        Some(ContentAction::Publish {
            title,
            summary,
            uri,
            mime,
            body,
            tags,
        }) => {
            guard_host_write(guard, &format!("content publish {title}"))?;
            let item = ContentItem {
                title: title.clone(),
                summary: summary.clone(),
                uri: uri.clone(),
                mime_type: mime.clone(),
                body: body.clone().into_bytes(),
                tags: tags.clone(),
                ..ContentItem::default()
            };
            let id = backend.publish(&item).map_err(SmartError::from)?;
            Ok(render::verb_value("publish", id, title))
        }
        Some(ContentAction::PublishMany {
            titles,
            summary,
            mime,
            body,
            tags,
        }) => {
            guard_host_write(guard, &format!("content publish-many ({} item(s))", titles.len()))?;
            let items: Vec<ContentItem> = titles
                .iter()
                .map(|title| ContentItem {
                    title: title.clone(),
                    summary: summary.clone(),
                    uri: String::new(),
                    mime_type: mime.clone(),
                    body: body.clone().into_bytes(),
                    tags: tags.clone(),
                    ..ContentItem::default()
                })
                .collect();
            let ids = backend.publish_many(&items).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("publish-many".into())),
                (
                    "ids".into(),
                    Value::List(ids.iter().map(|id| Value::Number(*id as f64)).collect()),
                ),
                ("count".into(), Value::Number(ids.len() as f64)),
            ]))
        }
        Some(ContentAction::Update {
            id,
            expected_version,
            title,
            summary,
            uri,
            mime,
            body,
            tags,
        }) => {
            guard_host_write(guard, &format!("content update {id}"))?;
            // The update carries the WHOLE item, so the current one is read
            // first and the named fields are replaced over it: an update that
            // names no field is a re-write of the same item, never a blank one.
            let current = backend.get(*id).map_err(|e| {
                match SmartError::from(e) {
                    SmartError::NotFound(_) => SmartError::NotFound(format!(
                        "the engine's store holds no item with id {id}"
                    )),
                    other => other,
                }
            })?;
            let item = ContentItem {
                id: current.id,
                title: title.clone().unwrap_or_else(|| current.title.clone()),
                summary: summary.clone().unwrap_or_else(|| current.summary.clone()),
                uri: uri.clone().unwrap_or_else(|| current.uri.clone()),
                mime_type: mime.clone().unwrap_or_else(|| current.mime_type.clone()),
                body: match body {
                    Some(text) => text.clone().into_bytes(),
                    None => current.body.clone(),
                },
                tags: tags.clone().unwrap_or_else(|| current.tags.clone()),
                ..current
            };
            let version = backend
                .update(*id, &item, *expected_version)
                .map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("update".into())),
                ("id".into(), Value::Number(*id as f64)),
                ("version".into(), Value::Number(version as f64)),
            ]))
        }
        Some(ContentAction::Unpublish { id }) => {
            guard_host_write(guard, &format!("content unpublish {id}"))?;
            backend.unpublish(*id).map_err(SmartError::from)?;
            Ok(render::verb_value("unpublish", *id, ""))
        }
        Some(ContentAction::Delete { id }) => {
            guard_host_write(guard, &format!("content delete {id}"))?;
            backend.delete(*id).map_err(SmartError::from)?;
            Ok(render::verb_value("delete", *id, ""))
        }
        Some(ContentAction::Trash { id }) => {
            guard_host_write(guard, &format!("content trash {id}"))?;
            backend.trash(*id).map_err(SmartError::from)?;
            Ok(render::verb_value("trash", *id, ""))
        }
        Some(ContentAction::Restore { id }) => {
            guard_host_write(guard, &format!("content restore {id}"))?;
            backend.restore(*id).map_err(SmartError::from)?;
            Ok(render::verb_value("restore", *id, ""))
        }
        Some(ContentAction::EmptyTrash) => {
            guard_host_write(guard, "content empty-trash")?;
            let emptied = backend.empty_trash().map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("empty-trash".into())),
                ("emptied".into(), Value::Number(emptied as f64)),
            ]))
        }
        Some(ContentAction::Caps) => Ok(render::caps_value(backend)),
        Some(ContentAction::Matrix) => Ok(render::matrix_value()),
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(
    cmd: &ContentCmd,
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
pub fn write_intent(cmd: &ContentCmd) -> Option<(String, String)> {
    use ContentAction as C;
    Some(match &cmd.action {
        Some(C::Publish { title, .. }) => ("publish content".into(), title.clone()),
        Some(C::PublishMany { titles, .. }) => {
            ("publish content".into(), format!("{} item(s)", titles.len()))
        }
        Some(C::Update { id, .. }) => ("update content".into(), format!("item {id}")),
        Some(C::Unpublish { id }) => ("unpublish content".into(), format!("item {id}")),
        Some(C::Delete { id }) => ("delete content".into(), format!("item {id}")),
        Some(C::Trash { id }) => ("trash content".into(), format!("item {id}")),
        Some(C::Restore { id }) => ("restore content".into(), format!("item {id}")),
        Some(C::EmptyTrash) => ("empty the content trash".into(), "".into()),
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
        "refusing to run `{verb}`: it CHANGES the engine's store. The interactive `(y/N)` prompt asks \
         with the target's context beside it; a piped `y` line answers it, and `--force` accepts \
         the write explicitly."
    )))
}

/// The content selector tree, rendered from the engine's own answers.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let items = render::list_value(backend, &ContentFilter::default(), page(0, u32::MAX))?;
    Ok(Value::Object(vec![
        ("total".into(), items.field("total").clone()),
        ("items".into(), items.field("items").clone()),
        ("caps".into(), render::caps_value(backend)),
    ]))
}

/// Re-export for the shell and main: the clap shape.
pub use self::{ContentAction as Action, ContentCmd as Cmd};
