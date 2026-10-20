//! `appindex` — the domain command module over the host's own desktop-entry
//! corpus.
//!
//! The engine's appindex cell reads the freedesktop `.desktop` files under the
//! XDG data directories (`src/platforms/linux/appindex.rs`) — the corpus the
//! host's own launcher reads. The shell renders from the engine's OWNED type
//! (`AppEntry`, `AppStats`, `AppSearchFilter`), so a row on screen is a row
//! the host published: 177 entries on this box, each with the name, the
//! generic name, the comment, the command line, the icon, the categories and
//! the keywords the entry really carries.
//!
//! ## What the corpus does not carry is not invented
//!
//! A desktop entry has no package id, no version, no developer and no
//! licence, so the shell shows no such column. The entry's SOURCE is the data
//! directory it came from — that is what the host actually publishes, and an
//! invented short label would be a claim the host never made.
//!
//! ## The pinned set is the engine's own
//!
//! `pin` / `unpin` change the engine's in-process pinned set, not the desktop
//! shell's own store (dconf), which this backend neither reads nor writes. The
//! rendering says so, so nobody reads a pinned row as "the host pinned it".
//!
//! ## Writes are gate-routed
//!
//! `pin` and `unpin` change the engine's set, so they carry a write intent and
//! are accepted the way every other mutating verb in this shell is accepted:
//! `--force` one-shot, a confirmation in the REPL. `launch` and `launchUrl`
//! are NOT verbs here: the engine does not serve them (they would start a
//! host process — law #8), and the `matrix` verb says so with the reason.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::appindex::types::{AppSearchFilter, Pagination};
use smart_lib::domains::appindex::AppindexBackend;

/// The engine seam this shell speaks to.
pub type Backend = dyn AppindexBackend;

/// The page window a command asks for.
fn page(offset: u32, limit: u32) -> Pagination {
    Pagination {
        offset,
        limit: limit.max(1),
    }
}

/// `smart appindex` — the host's own application index.
#[derive(Parser, Debug)]
pub struct AppindexCmd {
    #[command(subcommand)]
    pub action: Option<AppindexAction>,
}

#[derive(Subcommand, Debug)]
pub enum AppindexAction {
    /// The applications the host publishes (the default view).
    List {
        /// The first row to return.
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// How many rows to return.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// One application, by the id the host keys it under.
    Get {
        /// The entry's id — the desktop file's name, e.g. `firefox.desktop`.
        id: String,
    },
    /// The applications matching a filter.
    Search {
        /// A substring of the name, the generic name, the comment or a keyword.
        query: String,
        /// Only the entries carrying this category.
        #[arg(long)]
        category: Option<String>,
        /// Only the entries from this source directory.
        #[arg(long)]
        source: Option<String>,
        /// The first row to return.
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// How many rows to return.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// The index's own statistics: the entry count and the per-source and
    /// per-category counts.
    Stats,
    /// The applications the ENGINE has pinned (its own in-process set).
    Pinned,
    /// Pin an application in the engine's own set (a write — gate-routed).
    Pin {
        /// The entry's id.
        id: String,
    },
    /// Unpin an application from the engine's own set (a write — gate-routed).
    Unpin {
        /// The entry's id.
        id: String,
    },
    /// Several applications by their ids, in one call.
    Batch {
        /// The entries' ids.
        #[arg(required = true)]
        ids: Vec<String>,
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
pub fn evaluate(
    cmd: &AppindexCmd,
    backend: &Backend,
    guard: WriteGuard,
) -> SmartResult<Value> {
    match &cmd.action {
        None => Ok(render::list_value(backend, page(0, 50))?),
        Some(AppindexAction::List { offset, limit }) => {
            Ok(render::list_value(backend, page(*offset, *limit))?)
        }
        Some(AppindexAction::Get { id }) => {
            let entry = backend.get_app(id).map_err(|e| {
                match SmartError::from(e) {
                    SmartError::NotFound(_) => SmartError::NotFound(format!(
                        "the host publishes no desktop entry named {id}"
                    )),
                    other => other,
                }
            })?;
            Ok(render::entry_value(&entry))
        }
        Some(AppindexAction::Search {
            query,
            category,
            source,
            offset,
            limit,
        }) => {
            let filter = AppSearchFilter {
                name_contains: query.clone(),
                categories: category.iter().cloned().collect(),
                sources: source.iter().cloned().collect(),
            };
            Ok(render::search_value(backend, &filter, page(*offset, *limit))?)
        }
        Some(AppindexAction::Stats) => {
            let stats = backend.get_stats().map_err(SmartError::from)?;
            Ok(render::stats_value(&stats))
        }
        Some(AppindexAction::Pinned) => {
            let pinned = backend.get_pinned().map_err(SmartError::from)?;
            Ok(Value::List(pinned.iter().map(render::entry_value).collect()))
        }
        Some(AppindexAction::Pin { id }) => {
            guard_host_write(guard, &format!("appindex pin {id}"))?;
            backend.pin_app(id).map_err(SmartError::from)?;
            Ok(render::verb_value("pin", id))
        }
        Some(AppindexAction::Unpin { id }) => {
            guard_host_write(guard, &format!("appindex unpin {id}"))?;
            backend.unpin_app(id).map_err(SmartError::from)?;
            Ok(render::verb_value("unpin", id))
        }
        Some(AppindexAction::Batch { ids }) => {
            let entries = backend.batch_get(ids).map_err(SmartError::from)?;
            Ok(Value::List(entries.iter().map(render::entry_value).collect()))
        }
        Some(AppindexAction::Caps) => Ok(render::caps_value(backend)),
        Some(AppindexAction::Matrix) => Ok(render::matrix_value()),
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(
    cmd: &AppindexCmd,
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
pub fn write_intent(cmd: &AppindexCmd) -> Option<(String, String)> {
    use AppindexAction as A;
    Some(match &cmd.action {
        Some(A::Pin { id }) => ("pin application".into(), id.clone()),
        Some(A::Unpin { id }) => ("unpin application".into(), id.clone()),
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
        "refusing to run `{verb}`: it CHANGES the engine's own pinned set. The interactive `(y/N)` \
         prompt asks with the target's context beside it; a piped `y` line answers it, and \
         `--force` accepts the write explicitly."
    )))
}

/// The appindex selector tree, rendered from the engine's own answers.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let apps = render::list_value(backend, page(0, u32::MAX))?;
    Ok(Value::Object(vec![
        ("total".into(), apps.field("total").clone()),
        ("apps".into(), apps.field("apps").clone()),
        ("stats".into(), render::stats_value(&backend.get_stats()?)),
        ("caps".into(), render::caps_value(backend)),
    ]))
}

/// Re-export for the shell and main: the clap shape.
pub use self::{AppindexAction as Action, AppindexCmd as Cmd};
