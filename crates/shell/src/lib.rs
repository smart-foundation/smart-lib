//! The Smart Shell — the product. One binary, `smart`, hosting every domain
//! command over the smart-lib engine, in-process.
//!
//! ## The shape (the corrected architecture)
//!
//! ```text
//!    smart (this crate): REPL + the shell language + the aesthetic
//!         │  in-process, through the per-domain backend trait object
//!    smart-lib: the engine (canonical bindings + netlink/NM network)
//! ```
//!
//! The core (`shell`, `selector`, `pipe`, `alias`, `fuzzy`, `progress`,
//! `confirm`, `installer`, `complete`, `color`) is the original shell's
//! engine-agnostic half, salvaged: it touches no schema and no contract type.
//! The domain modules render from the engine's OWNED types. Domains whose
//! engine half does not exist yet are not faked — they answer honestly from
//! the migration queue ([`crate::migrate`]).

pub mod alias;
pub mod color;
pub mod complete;
pub mod confirm;
pub mod domains;
pub mod error;
pub mod fuzzy;
pub mod history;
pub mod installer;
pub mod migrate;
pub mod pipe;
pub mod progress;
pub mod selector;
pub mod shell;
pub mod value;
pub mod widgets;

pub use error::{SmartError, SmartResult};
pub use selector::Value;

/// The output shape, kept exactly as the original shell had it: the aesthetic
/// for humans, JSON for agents and scripts, Raw for pipes and machines.
/// Machine output rides alongside the aesthetic, never replacing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum OutputFormat {
    Human,
    Json,
    Raw,
}
