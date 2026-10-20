//! `queue` — the domain command module over the engine's own FIFO store.
//!
//! The engine's queue cell is an in-process FIFO (`src/platforms/linux/queue.rs`):
//! named queues of byte payloads, behind one mutex, with the contract's own
//! `enqueue` / `dequeue` / `peek` / `clearQueue` / `deleteQueue` surface. The
//! shell speaks only that surface — the engine's bytes in, the engine's bytes
//! out — never a capnp reader.
//!
//! ## What the engine does not expose, and the shell does not invent
//!
//! The queue's DEPTH is not part of the contract's read surface: the engine
//! answers "which queues exist" and "what is at the head", and the shell
//! reports exactly that. A depth column would be a number the engine never
//! measured.
//!
//! ## Writes are gate-routed
//!
//! `enqueue`, `dequeue`, `clear` and `delete` change the store, so they carry
//! a write intent and are accepted the way every other mutating verb in this
//! shell is accepted: `--force` one-shot, a confirmation in the REPL.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::queue::QueueBackend;

/// The engine seam this shell speaks to.
pub type Backend = dyn QueueBackend;

/// `smart queue` — the engine's own FIFO store.
#[derive(Parser, Debug)]
pub struct QueueCmd {
    #[command(subcommand)]
    pub action: Option<QueueAction>,
}

#[derive(Subcommand, Debug)]
pub enum QueueAction {
    /// The queues the store holds (the default view).
    List,
    /// Whether a queue exists, and the name the store holds it under.
    Get {
        /// The queue's name.
        name: String,
    },
    /// The payload at the head of a queue, without removing it.
    Peek {
        /// The queue's name.
        name: String,
    },
    /// Append a payload (a write — gate-routed).
    Enqueue {
        /// The queue to append to.
        name: String,
        /// The payload, as text; the engine stores its UTF-8 bytes.
        data: String,
    },
    /// Take the payload at the head of a queue (a write — gate-routed).
    Dequeue {
        /// The queue to take from.
        name: String,
    },
    /// Empty a queue, keeping the queue itself (a write — gate-routed).
    Clear {
        /// The queue to empty.
        name: String,
    },
    /// Remove a queue and everything in it (a write — gate-routed).
    Delete {
        /// The queue to remove.
        name: String,
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
pub fn evaluate(cmd: &QueueCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None | Some(QueueAction::List) => Ok(render::list_value(backend)?),
        Some(QueueAction::Get { name }) => {
            let held = backend.get_queue(name).map_err(|e| {
                match SmartError::from(e) {
                    SmartError::NotFound(_) => {
                        SmartError::NotFound(format!("the engine's store holds no queue named {name}"))
                    }
                    other => other,
                }
            })?;
            Ok(Value::Object(vec![
                ("name".into(), Value::String(held)),
                ("exists".into(), Value::Bool(true)),
            ]))
        }
        Some(QueueAction::Peek { name }) => {
            // The engine answers `None` for an empty queue AND for a queue that
            // is not there, so the existence is read first: the shell never
            // calls an absent queue "empty".
            let exists = backend.get_queue(name).is_ok();
            let head = backend.peek(name).map_err(SmartError::from)?;
            Ok(render::payload_record(name, exists, head.as_deref(), false))
        }
        Some(QueueAction::Enqueue { name, data }) => {
            guard_host_write(guard, &format!("queue enqueue {name}"))?;
            let id = backend.enqueue(name, data.as_bytes()).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("enqueue".into())),
                ("name".into(), Value::String(name.clone())),
                ("id".into(), Value::Number(id as f64)),
                ("size".into(), Value::Number(data.len() as f64)),
            ]))
        }
        Some(QueueAction::Dequeue { name }) => {
            guard_host_write(guard, &format!("queue dequeue {name}"))?;
            let payload = backend.dequeue(name).map_err(SmartError::from)?;
            Ok(render::payload_record(name, true, payload.as_deref(), true))
        }
        Some(QueueAction::Clear { name }) => {
            guard_host_write(guard, &format!("queue clear {name}"))?;
            backend.clear_queue(name).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("clear".into())),
                ("name".into(), Value::String(name.clone())),
            ]))
        }
        Some(QueueAction::Delete { name }) => {
            guard_host_write(guard, &format!("queue delete {name}"))?;
            backend.delete_queue(name).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("delete".into())),
                ("name".into(), Value::String(name.clone())),
            ]))
        }
        Some(QueueAction::Caps) => Ok(render::caps_value(backend)),
        Some(QueueAction::Matrix) => Ok(render::matrix_value()),
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(
    cmd: &QueueCmd,
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
pub fn write_intent(cmd: &QueueCmd) -> Option<(String, String)> {
    use QueueAction as Q;
    Some(match &cmd.action {
        Some(Q::Enqueue { name, .. }) => ("enqueue on queue".into(), name.clone()),
        Some(Q::Dequeue { name }) => ("dequeue from queue".into(), name.clone()),
        Some(Q::Clear { name }) => ("clear queue".into(), name.clone()),
        Some(Q::Delete { name }) => ("delete queue".into(), name.clone()),
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

/// The queue selector tree, rendered from the engine's own answers.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    Ok(Value::Object(vec![
        ("queues".into(), render::list_value(backend)?),
        ("caps".into(), render::caps_value(backend)),
    ]))
}

/// Re-export for the shell and main: the clap shape.
pub use self::{QueueAction as Action, QueueCmd as Cmd};
