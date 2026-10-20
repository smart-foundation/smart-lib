//! `manifest` — the domain command module over the engine's own manifest store.
//!
//! The engine's manifest cell is an in-process store of manifests
//! (`src/platforms/linux/manifest.rs`): the contract's manifests describe the
//! engine's own bundles, and this host publishes no bundle corpus of its own,
//! so the store *is* the mechanism. The shell renders from the engine's OWNED
//! types (`ManifestEntry`, `ValidationReport`).
//!
//! ## The store's write path
//!
//! The contract has no `put` verb: manifests arrive with the bundles the
//! engine installs. The engine's cell exposes that write path as an inherent
//! method (`put` / `remove`), so the shell declares a small store trait over
//! the engine's own trait and registers a manifest with `manifest register`.
//! Without that verb the domain would be a read-only view of a store nothing
//! could fill — half a domain, and the founder's rule is no half-baking.
//!
//! ## The validation is a real check
//!
//! `validate` / `validate-deps` ask the engine's own validator: a REQUIRED
//! dependency that no manifest in the store offers is an error the caller is
//! told about; an optional one is not. The shell prints the engine's errors
//! verbatim and never turns a red report green.
//!
//! ## Writes are gate-routed
//!
//! `register` and `unregister` change the store, so they carry a write intent
//! and are accepted the way every other mutating verb in this shell is
//! accepted: `--force` one-shot, a confirmation in the REPL.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::manifest::types::{ManifestDependency, ManifestEntry, Pagination};
use smart_lib::domains::manifest::ManifestBackend;

/// The engine seam this shell speaks to — the engine's manifest trait PLUS the
/// store's own write path, which the engine's cell exposes as inherent
/// methods rather than as contract verbs.
pub trait ManifestStore: ManifestBackend {
    /// Register a manifest in the store.
    fn register(&self, manifest: ManifestEntry) -> SmartResult<()>;
    /// Remove a manifest from the store.
    fn unregister(&self, name: &str) -> SmartResult<()>;
}

/// The engine's Linux cell, taught the store's two write verbs: the engine
/// exposes them as inherent methods (`put` / `remove`), and the shell reaches
/// them through its own trait so the command module keeps a `dyn` seam and a
/// test seam at the same time.
impl ManifestStore
    for smart_lib::platforms::linux::manifest::LinuxManifestBackend
{
    fn register(&self, manifest: ManifestEntry) -> SmartResult<()> {
        smart_lib::platforms::linux::manifest::LinuxManifestBackend::put(self, manifest)
            .map_err(SmartError::from)
    }
    fn unregister(&self, name: &str) -> SmartResult<()> {
        smart_lib::platforms::linux::manifest::LinuxManifestBackend::remove(self, name)
            .map_err(SmartError::from)
    }
}

/// The engine seam as the shell uses it: a trait object over the store trait.
pub type Backend = dyn ManifestStore;

/// The page window a command asks for.
fn page(offset: u32, limit: u32) -> Pagination {
    Pagination {
        offset,
        limit: limit.max(1),
    }
}

/// `smart manifest` — the engine's own manifest store.
#[derive(Parser, Debug)]
pub struct ManifestCmd {
    #[command(subcommand)]
    pub action: Option<ManifestAction>,
}

#[derive(Subcommand, Debug)]
pub enum ManifestAction {
    /// The manifests the store holds (the default view).
    List {
        /// The first row to return.
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// How many rows to return.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// One manifest, by the name it is filed under.
    Resolve {
        /// The manifest's name.
        name: String,
    },
    /// One manifest, by its human spelling, scoped by the vendor that issued
    /// it (an empty vendor matches any vendor).
    ByName {
        /// The vendor that issued it (empty matches any vendor).
        issuer: String,
        /// The manifest's display name.
        name: String,
    },
    /// Validate a manifest against the store.
    Validate {
        /// The manifest's name.
        name: String,
    },
    /// Validate a manifest's dependencies against the store.
    ValidateDeps {
        /// The manifest's name.
        name: String,
    },
    /// Register a manifest in the store (a write — gate-routed).
    Register {
        /// The manifest's name — the opaque id it is filed under.
        name: String,
        /// The human-readable label.
        #[arg(long, default_value = "")]
        display: String,
        /// The description.
        #[arg(long, default_value = "")]
        description: String,
        /// The vendor.
        #[arg(long, default_value = "")]
        vendor: String,
        /// The semantic version (`1.2.3`).
        #[arg(long, default_value = "1.0.0")]
        version: String,
        /// An entry point (repeatable).
        #[arg(long = "entry")]
        entries: Vec<String>,
        /// A schema the manifest offers (repeatable).
        #[arg(long = "offers")]
        offers: Vec<String>,
        /// A dependency: `name` or `name:optional`.
        #[arg(long = "depends")]
        depends: Vec<String>,
        /// The bundle shape, as an open token.
        #[arg(long, default_value = "")]
        shape: String,
    },
    /// Remove a manifest from the store (a write — gate-routed).
    Unregister {
        /// The manifest's name.
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
pub fn evaluate(
    cmd: &ManifestCmd,
    backend: &Backend,
    guard: WriteGuard,
) -> SmartResult<Value> {
    match &cmd.action {
        None => Ok(render::list_value(backend, page(0, 50))?),
        Some(ManifestAction::List { offset, limit }) => {
            Ok(render::list_value(backend, page(*offset, *limit))?)
        }
        Some(ManifestAction::Resolve { name }) => {
            let manifest = backend.resolve(name).map_err(|e| {
                match SmartError::from(e) {
                    SmartError::NotFound(_) => SmartError::NotFound(format!(
                        "the engine's store holds no manifest named {name}"
                    )),
                    other => other,
                }
            })?;
            Ok(render::manifest_value(&manifest))
        }
        Some(ManifestAction::ByName { issuer, name }) => {
            let manifest = backend
                .resolve_by_name(issuer, name)
                .map_err(SmartError::from)?;
            Ok(render::manifest_value(&manifest))
        }
        Some(ManifestAction::Validate { name }) => {
            let report = backend.validate(name).map_err(|e| {
                match SmartError::from(e) {
                    SmartError::NotFound(_) => SmartError::NotFound(format!(
                        "the engine's store holds no manifest named {name}"
                    )),
                    other => other,
                }
            })?;
            Ok(render::validation_value(&report, name, "validate"))
        }
        Some(ManifestAction::ValidateDeps { name }) => {
            let report = backend
                .validate_dependencies(name)
                .map_err(SmartError::from)?;
            Ok(render::validation_value(&report, name, "validate-dependencies"))
        }
        Some(ManifestAction::Register {
            name,
            display,
            description,
            vendor,
            version,
            entries,
            offers,
            depends,
            shape,
        }) => {
            guard_host_write(guard, &format!("manifest register {name}"))?;
            let manifest = ManifestEntry {
                name: name.clone(),
                display_name: display.clone(),
                description: description.clone(),
                vendor: vendor.clone(),
                version: version.clone(),
                entries: entries.clone(),
                depends: depends.iter().map(|spec| dependency_of(spec)).collect(),
                offers: offers.clone(),
                shape: shape.clone(),
            };
            backend.register(manifest)?;
            Ok(render::verb_value("register", name))
        }
        Some(ManifestAction::Unregister { name }) => {
            guard_host_write(guard, &format!("manifest unregister {name}"))?;
            backend.unregister(name)?;
            Ok(render::verb_value("unregister", name))
        }
        Some(ManifestAction::Caps) => Ok(render::caps_value(backend)),
        Some(ManifestAction::Matrix) => Ok(render::matrix_value()),
    }
}

/// One dependency as the shell spells it on the command line: `name`, or
/// `name:optional` for a dependency the store may lack without failing the
/// validation.
fn dependency_of(spec: &str) -> ManifestDependency {
    match spec.split_once(':') {
        Some((name, flag)) => ManifestDependency {
            schema_id: 0,
            name: name.to_string(),
            optional: flag.eq_ignore_ascii_case("optional") || flag == "1",
        },
        None => ManifestDependency {
            schema_id: 0,
            name: spec.to_string(),
            optional: false,
        },
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(
    cmd: &ManifestCmd,
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
pub fn write_intent(cmd: &ManifestCmd) -> Option<(String, String)> {
    use ManifestAction as M;
    Some(match &cmd.action {
        Some(M::Register { name, .. }) => ("register manifest".into(), name.clone()),
        Some(M::Unregister { name }) => ("unregister manifest".into(), name.clone()),
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

/// The manifest selector tree, rendered from the engine's own answers.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let manifests = render::list_value(backend, page(0, u32::MAX))?;
    Ok(Value::Object(vec![
        ("total".into(), manifests.field("total").clone()),
        ("manifests".into(), manifests.field("manifests").clone()),
        ("caps".into(), render::caps_value(backend)),
    ]))
}

/// Re-export for the shell and main: the clap shape.
pub use self::{ManifestAction as Action, ManifestCmd as Cmd};
