//! `database` — the domain command module over the engine's own store.
//!
//! The engine's database cell is an in-process, namespaced key/value store
//! (`src/platforms/linux/database.rs`): namespaces of keys to byte payloads,
//! behind one mutex, with the contract's own `listKeys` / `listNamespaces`
//! paging. The shell speaks only that surface — the engine's OWNED
//! `Pagination` in, the engine's bytes out — never a capnp reader, never a
//! `Box::leak`.
//!
//! ## The store's lifetime is the engine's own
//!
//! The store lives in the process that built it, so the shell's session keeps
//! ONE backend per process (the rule the files/process/settings domains
//! already follow): `set → get` composes across the lines of one session.
//! A one-shot invocation is one session, and the store it reads is the store
//! that process built — the shell says so in the rendering rather than
//! pretending a corpus it does not have.
//!
//! ## Writes are gate-routed
//!
//! `set`, `delete` and `clear` change the store, so they carry a write
//! intent and are accepted the way every other mutating verb in this shell is
//! accepted: `--force` one-shot, a confirmation in the REPL. The refusal names
//! the verb and the target, and never the value (a stored value is not echoed
//! back by a refusal).

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::database::DatabaseBackend;
use smart_lib::domains::database::types::Pagination;

/// The engine seam this shell speaks to — the corrected architecture's rule:
/// the shell embeds its engine in-process through the per-domain backend
/// trait object.
pub type Backend = dyn DatabaseBackend;

/// The page window a command asks for. The engine's own `effective_limit`
/// reads a zero limit as ONE row, so the shell never asks for zero.
fn page(offset: u32, limit: u32) -> Pagination {
    Pagination {
        offset,
        limit: limit.max(1),
    }
}

/// `smart database` — the engine's own namespaced key/value store.
#[derive(Parser, Debug)]
pub struct DatabaseCmd {
    #[command(subcommand)]
    pub action: Option<DatabaseAction>,
}

#[derive(Subcommand, Debug)]
pub enum DatabaseAction {
    /// The namespaces the store holds (the default view).
    Namespaces {
        /// The first row to return.
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// How many rows to return.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// The keys of one namespace, optionally under a prefix.
    Keys {
        /// The namespace to list.
        namespace: String,
        /// Only the keys that start with this prefix.
        #[arg(long, default_value = "")]
        prefix: String,
        /// The first row to return.
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// How many rows to return.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// One value, read from the store.
    Get {
        /// The namespace the key lives in.
        namespace: String,
        /// The key.
        key: String,
    },
    /// Whether a key is present.
    Has {
        /// The namespace the key would live in.
        namespace: String,
        /// The key.
        key: String,
    },
    /// How many keys a namespace holds.
    Count {
        /// The namespace to count.
        namespace: String,
    },
    /// Store a value (a write — gate-routed).
    Set {
        /// The namespace to write into.
        namespace: String,
        /// The key.
        key: String,
        /// The value, as text; the engine stores its UTF-8 bytes.
        value: String,
    },
    /// Remove one key (a write — gate-routed).
    Delete {
        /// The namespace the key lives in.
        namespace: String,
        /// The key.
        key: String,
    },
    /// Remove every key of a namespace (a write — gate-routed).
    Clear {
        /// The namespace to empty.
        namespace: String,
    },
    /// The backend's own capability tokens.
    Caps,
    /// The contract's availability matrix, exactly as the engine publishes it.
    Matrix,
}

/// Whether the caller accepted the write (the REPL prompts; one-shot mode
/// passes the `--force` flag through).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard {
    pub allowed: bool,
}

/// Evaluate the command's data half: the value the command produces, with no
/// rendering. This is what pipes consume.
pub fn evaluate(cmd: &DatabaseCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None => Ok(render::namespaces_value(backend, page(0, 50))?),
        Some(DatabaseAction::Namespaces { offset, limit }) => {
            Ok(render::namespaces_value(backend, page(*offset, *limit))?)
        }
        Some(DatabaseAction::Keys {
            namespace,
            prefix,
            offset,
            limit,
        }) => Ok(render::keys_value(
            backend,
            namespace,
            prefix,
            page(*offset, *limit),
        )?),
        Some(DatabaseAction::Get { namespace, key }) => {
            let bytes = backend.get(namespace, key).map_err(|e| {
                // The engine's "not found" with the TARGET named: a bare
                // `notFound` says nothing about WHICH key was asked for.
                match SmartError::from(e) {
                    SmartError::NotFound(_) => SmartError::NotFound(format!(
                        "no value under {namespace}/{key} in the engine's store"
                    )),
                    other => other,
                }
            })?;
            Ok(render::value_record(namespace, key, &bytes))
        }
        Some(DatabaseAction::Has { namespace, key }) => {
            let present = backend.has(namespace, key).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("namespace".into(), Value::String(namespace.clone())),
                ("key".into(), Value::String(key.clone())),
                ("present".into(), Value::Bool(present)),
            ]))
        }
        Some(DatabaseAction::Count { namespace }) => {
            let count = backend.count(namespace).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("namespace".into(), Value::String(namespace.clone())),
                ("count".into(), Value::Number(count as f64)),
            ]))
        }
        Some(DatabaseAction::Set {
            namespace,
            key,
            value,
        }) => {
            guard_host_write(guard, &format!("database set {namespace}/{key}"))?;
            backend
                .set(namespace, key, value.as_bytes())
                .map_err(SmartError::from)?;
            Ok(render::verb_value("set", namespace, key, value.len()))
        }
        Some(DatabaseAction::Delete { namespace, key }) => {
            guard_host_write(guard, &format!("database delete {namespace}/{key}"))?;
            backend.delete(namespace, key).map_err(|e| {
                match SmartError::from(e) {
                    SmartError::NotFound(_) => SmartError::NotFound(format!(
                        "no value under {namespace}/{key} in the engine's store"
                    )),
                    other => other,
                }
            })?;
            Ok(render::verb_value("delete", namespace, key, 0))
        }
        Some(DatabaseAction::Clear { namespace }) => {
            guard_host_write(guard, &format!("database clear {namespace}"))?;
            backend.clear(namespace).map_err(SmartError::from)?;
            Ok(render::verb_value("clear", namespace, "*", 0))
        }
        Some(DatabaseAction::Caps) => Ok(render::caps_value(backend)),
        Some(DatabaseAction::Matrix) => Ok(render::matrix_value()),
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(
    cmd: &DatabaseCmd,
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

/// The write this command performs, when it performs one: the verb and the
/// target the confirmation prompt shows. A read verb answers `None` and is
/// never asked anything.
pub fn write_intent(cmd: &DatabaseCmd) -> Option<(String, String)> {
    use DatabaseAction as D;
    Some(match &cmd.action {
        Some(D::Set { namespace, key, .. }) => ("set database value".into(), format!("{namespace}/{key}")),
        Some(D::Delete { namespace, key }) => ("delete database value".into(), format!("{namespace}/{key}")),
        Some(D::Clear { namespace }) => ("clear database namespace".into(), namespace.clone()),
        _ => return None,
    })
}

pub mod render;

/// The host-write gate (law #8's shape): a mutating verb proceeds only when
/// the caller accepted the write.
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

/// The database selector tree, rendered from the engine's own answers — the
/// same values the command surface renders, so a selector and a command can
/// never disagree. `@database/namespaces`, `@database/keys` resolve from here.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let namespaces = render::namespaces_value(backend, page(0, u32::MAX))?;
    Ok(Value::Object(vec![
        ("total".into(), namespaces.field("total").clone()),
        ("namespaces".into(), namespaces.field("namespaces").clone()),
        ("caps".into(), render::caps_value(backend)),
    ]))
}

/// Re-export for the shell and main: the clap shape.
pub use self::{DatabaseAction as Action, DatabaseCmd as Cmd};
