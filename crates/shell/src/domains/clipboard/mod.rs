//! `clipboard` — the domain command module, rewritten onto the NEW engine.
//!
//! What changed from the original shell's module: it renders from the engine's
//! OWNED types (`ContentOutcome`, `ClipboardItem`, `ClipboardPreview`, …)
//! instead of capnp readers, and its read path speaks the selection protocol
//! natively over the engine's Linux backend (`x11rb` on this host) — the old
//! shell's `wl-paste`/`xclip` subprocess shellout is gone (there is no
//! subprocess anywhere in this domain). Host-mutating verbs (`set`, `clear`,
//! the history mutators) are gated exactly as `network`'s are: the operator must
//! accept the write (`--force` one-shot, or confirm in the REPL); the live
//! bring-back proof of a write is the founder's (charter law #8).

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::clipboard::ClipboardBackend;
use smart_lib::domains::clipboard::types::Pagination;

/// The engine seam this shell speaks to — the corrected architecture's rule: the
/// shell embeds its engine in-process through the per-domain backend trait
/// object. Without the feature the seam is the unit type, so every signature
/// still compiles (and every path answers honestly that the engine half is not
/// built).
pub type Backend = dyn ClipboardBackend;

/// `smart clipboard` — the one system clipboard's content, the engine-side
/// history ledger, and the mutating verbs.
#[derive(Parser, Debug)]
pub struct ClipboardCmd {
    #[command(subcommand)]
    pub action: Option<ClipboardAction>,
}

#[derive(Subcommand, Debug)]
pub enum ClipboardAction {
    /// The clipboard's current state: transport, content preview, history size.
    Status,
    /// Read the current clipboard content for a MIME type (the default `--mime`
    /// is `text/plain`).
    Get {
        #[arg(long, default_value = "text/plain")]
        mime: String,
    },
    /// Set the clipboard content (host-mutating — gated; founder-run live).
    Set {
        /// The text to place on the clipboard.
        text: String,
        #[arg(long, default_value = "text/plain")]
        mime: String,
    },
    /// Clear the current clipboard content (host-mutating — gated; founder-run).
    Clear,
    /// The engine-side copy-history ledger.
    History {
        #[command(subcommand)]
        action: Option<HistoryAction>,
    },
    /// The MIME types the current selection offers.
    Types,
    /// A low-resolution preview of one history item.
    Preview { id: u64 },
    /// Validate that a history item's content matches its claimed MIME type.
    Validate { id: u64 },
    /// Convert a history item to a target MIME type.
    Convert { id: u64, mime: String },
    /// The backend's declared size limits.
    Limits,
}

#[derive(Subcommand, Debug)]
pub enum HistoryAction {
    /// Search the ledger.
    Search { query: String },
    /// Pin an item against auto-expiry (host-mutating — gated).
    Pin { id: u64 },
    /// Unpin a pinned item (host-mutating — gated).
    Unpin { id: u64 },
    /// Remove one item from the ledger (host-mutating — gated).
    Delete { id: u64 },
}

/// Whether the caller accepted the host write (the REPL prompts; one-shot mode
/// passes the `--force` flag through) — the C1 gate, re-used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard {
    pub allowed: bool,
}

/// Every page read this command module issues asks for everything (the shell
/// slices with the pipe engine); the backend still bounds each pull.
fn all() -> Pagination {
    Pagination {
        offset: 0,
        limit: u32::MAX,
    }
}

/// Evaluate the command's data half: the value the command produces, with no
/// rendering. This is what pipes consume.
pub fn evaluate(cmd: &ClipboardCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None | Some(ClipboardAction::Status) => render::status_value(backend),
        Some(ClipboardAction::Get { mime }) => match backend.get_content(mime)? {
            smart_lib::domains::clipboard::types::ContentOutcome::Content(c) => {
                Ok(render::content_value(&c.mime_type, &c.data))
            }
            smart_lib::domains::clipboard::types::ContentOutcome::Empty => Ok(Value::Null),
        },
        Some(ClipboardAction::Set { text, .. }) => {
            guard_host_write(guard, "clipboard set")?;
            backend.set_content(smart_lib::domains::clipboard::types::ClipboardContent {
                mime_type: "text/plain".to_string(),
                data: text.clone().into_bytes(),
            })?;
            Ok(render::verb_value(
                "setContent",
                &format!("{} bytes placed on the clipboard", text.len()),
            ))
        }
        Some(ClipboardAction::Clear) => {
            guard_host_write(guard, "clipboard clear")?;
            backend.clear_content()?;
            Ok(render::verb_value("clearContent", "the selection was relinquished"))
        }
        Some(ClipboardAction::History { action }) => match action {
            None => render::history_value(backend, all()),
            Some(HistoryAction::Search { query }) => render::search_value(backend, query, all()),
            Some(HistoryAction::Pin { id }) => {
                guard_host_write(guard, "clipboard history pin")?;
                backend.pin_item(backend_id(*id))?;
                Ok(render::verb_value("pinItem", &format!("item {id} pinned")))
            }
            Some(HistoryAction::Unpin { id }) => {
                guard_host_write(guard, "clipboard history unpin")?;
                backend.unpin_item(backend_id(*id))?;
                Ok(render::verb_value("unpinItem", &format!("item {id} unpinned")))
            }
            Some(HistoryAction::Delete { id }) => {
                guard_host_write(guard, "clipboard history delete")?;
                backend.delete_item(backend_id(*id))?;
                Ok(render::verb_value("deleteItem", &format!("item {id} removed")))
            }
        },
        Some(ClipboardAction::Types) => render::mimes_value(backend),
        Some(ClipboardAction::Preview { id }) => render::preview_value(backend, *id),
        Some(ClipboardAction::Validate { id }) => match backend.validate_content(backend_id(*id))? {
            smart_lib::domains::clipboard::types::ValidationOutcome::Valid => Ok(render::verb_value(
                "validateContent",
                "the content matches its claimed MIME type",
            )),
            smart_lib::domains::clipboard::types::ValidationOutcome::Invalid(r) => {
                Ok(render::verb_value("validateContent", &format!("INVALID — {r}")))
            }
            smart_lib::domains::clipboard::types::ValidationOutcome::Unknown => {
                Ok(render::verb_value("validateContent", "the backend could not classify it"))
            }
        },
        Some(ClipboardAction::Convert { id, mime }) => match backend.convert_content(backend_id(*id), mime)? {
            smart_lib::domains::clipboard::types::ConversionOutcome::Content(c) => {
                Ok(render::content_value(&c.mime_type, &c.data))
            }
            smart_lib::domains::clipboard::types::ConversionOutcome::Unsupported => Ok(render::verb_value(
                "convertContent",
                &format!("no conversion to {mime} is offered by this backend"),
            )),
        },
        Some(ClipboardAction::Limits) => render::limits_value(backend),
    }
}

fn backend_id(id: u64) -> smart_lib::core::ids::ClipboardId {
    smart_lib::core::ids::ClipboardId::new(id)
}

/// Run the command: evaluate, render in the requested format.
pub fn run(cmd: &ClipboardCmd, format: &OutputFormat, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    let value = evaluate(cmd, backend, guard)?;
    if *format != OutputFormat::Raw {
        println!("{}", render::render_action(cmd, &value, *format));
    }
    Ok(value)
}

pub mod render;

/// The host-write gate (law #8, the C1 shape): a mutating verb proceeds only
/// when the caller accepted the write.
fn guard_host_write(guard: WriteGuard, verb: &str) -> SmartResult<()> {
    if guard.allowed {
        return Ok(());
    }
    Err(SmartError::not_supported(format!(
        "refusing to run `{verb}`: it CHANGES host state. In the REPL confirm the prompt, or pass \
         --force once you accept the write (the live bring-back proof is founder-run)."
    )))
}

/// The clipboard selector tree, rendered from the engine's owned types — the
/// same values the command surface renders, so a selector and a command can
/// never disagree. `@clipboard/content`, `@clipboard/mimes`,
/// `@clipboard/history` resolve from here.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    render::status_value(backend)
}

/// Re-export for the shell and main: the clap shape.
pub use self::{ClipboardAction as Action, ClipboardCmd as Cmd};
