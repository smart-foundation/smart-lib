//! `settings` — the domain command module on the NEW engine.
//!
//! It renders from the engine's OWNED types (`SettingResult`, `SettingValue`,
//! `SchemaInfo`, `SchemaKey`, `SettingChange`) and reads the host's own settings
//! engine natively — glib's `GSettings` machinery through the maintained `gio`
//! crate, the very engine the host's own `gsettings` is a thin CLI over: the
//! compiled schema source for schemas, defaults, ranges and summaries, the
//! host's own dconf database for per-user values, and the host's own `changed`
//! signal for the watch surface. There is no `gsettings` fork and no `dconf`
//! fork anywhere in this domain.
//!
//! ## The value model is the contract's own, and `notSet` is an answer
//!
//! `get` returns the contract's total union, so the shell never paints a
//! default as a value: a key the host never set answers `notSet` with the
//! schema's own default printed BESIDE it, named as the default.
//!
//! ## The writes are gated, once
//!
//! `set`/`delete`/`clear`/`bulkSet` go through the host's own engine and are
//! gated exactly as every other host-mutating verb is (charter law #8: the
//! write path is code-complete and the live bring-back proof is founder-run).
//! What this host cannot serve is answered with the measured reason: the
//! contract's category tokens are conventions and glib publishes no per-key
//! category, and this host's engine has no per-app namespace.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::settings::backend::render_value;
use smart_lib::domains::settings::types::{
    Pagination, SchemaInfo, SchemaKey, SettingChange, SettingResult, SettingValue,
};
use smart_lib::domains::settings::SettingsBackend;

/// The engine seam this shell speaks to — the per-domain backend trait object.
pub type Backend = dyn SettingsBackend;

/// Every page read this command module issues asks for everything (the shell
/// slices with the pipe engine); the backend still bounds each pull.
fn all() -> Pagination {
    Pagination {
        offset: 0,
        limit: u32::MAX,
    }
}

/// `smart settings` — the host's own configuration, read through its own
/// settings engine.
#[derive(Parser, Debug)]
pub struct SettingsCmd {
    #[command(subcommand)]
    pub action: Option<SettingsAction>,
}

#[derive(Subcommand, Debug)]
pub enum SettingsAction {
    /// The schema source the host publishes (the default view): every domain
    /// with its own key count.
    Domains,
    /// One domain's live values (`getAll` — the contract's StringMap).
    List {
        /// The schema id, e.g. `org.gnome.desktop.interface`.
        domain: String,
    },
    /// Read one key (`get`) — the contract's own union, so a missing key is an
    /// answer and not a sentinel.
    Get {
        /// The schema id.
        domain: String,
        /// The key inside the schema.
        key: String,
    },
    /// Write one key (host-mutating — gated; the host's own dconf path is
    /// code-complete for the founder's accepted write).
    Set {
        /// The schema id.
        domain: String,
        /// The key inside the schema.
        key: String,
        /// The value, read through the schema's own type when it declares one.
        value: String,
    },
    /// The schema browser: one schema's keys with the host's own types,
    /// defaults, ranges and summaries.
    Schema {
        /// The schema id.
        domain: String,
    },
    /// The contract's own category listing (`listByCategory`) — the category is
    /// an OPEN token, recorded beside each key rather than guessed.
    Category {
        /// The category token: security, display, accessibility, network,
        /// system, user, audio, privacy, developer.
        category: String,
    },
    /// The store's own version of one key (`getVersion`).
    Version {
        /// The schema id.
        domain: String,
        /// The key inside the schema.
        key: String,
    },
    /// One key's recorded change history (`getHistory`).
    History {
        /// The schema id.
        domain: String,
        /// The key inside the schema.
        key: String,
    },
    /// The delta read instead of a full-state poll (`getSettingsDiff`).
    Diff {
        /// The schema id.
        domain: String,
        /// The version to read after.
        #[arg(long, default_value = "0")]
        since: u64,
    },
    /// Read several keys at once (`bulkGet` — the contract's StringMap read).
    BulkGet {
        /// The schema id.
        domain: String,
        /// The keys to read.
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Write several keys at once (host-mutating — gated; the StringMap write).
    BulkSet {
        /// The schema id.
        domain: String,
        /// `key=value` pairs.
        #[arg(required = true)]
        entries: Vec<String>,
    },
    /// The host's own change signal, delivered live and bounded by the caller.
    Watch {
        /// The schema id to watch.
        domain: String,
        /// Only this key's changes.
        #[arg(long)]
        key: Option<String>,
        /// How many events to take before stopping.
        #[arg(long, default_value = "3")]
        count: u32,
        /// The pause between pulls, in milliseconds.
        #[arg(long, default_value = "400")]
        interval: u64,
    },
    /// This backend's honest capability tokens.
    Caps,
}

/// Whether the caller accepted the host write (the REPL prompts; one-shot mode
/// passes the `--force` flag through) — the C1 gate, re-used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard {
    pub allowed: bool,
}

/// Evaluate the command's data half: the value the command produces, with no
/// rendering. This is what pipes consume.
pub fn evaluate(cmd: &SettingsCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None | Some(SettingsAction::Domains) => render::domains_value(backend),
        Some(SettingsAction::List { domain }) => render::list_value(backend, domain),
        Some(SettingsAction::Get { domain, key }) => render::get_value(backend, domain, key),
        Some(SettingsAction::Set { domain, key, value }) => {
            guard_host_write(guard, &format!("settings set {domain} {key}"))?;
            let typed = render::typed_value(backend, domain, key, value)?;
            backend
                .set(domain, key, &typed)
                .map_err(SmartError::from)?;
            Ok(render::verb_value("set", &format!("{domain} {key}")))
        }
        Some(SettingsAction::Schema { domain }) => render::schema_value(backend, domain),
        Some(SettingsAction::Category { category }) => render::category_value(backend, category),
        Some(SettingsAction::Version { domain, key }) => {
            let version = backend.get_version(domain, key).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("domain".into(), Value::String(domain.clone())),
                ("key".into(), Value::String(key.clone())),
                ("version".into(), Value::Number(version as f64)),
            ]))
        }
        Some(SettingsAction::History { domain, key }) => render::history_value(backend, domain, key),
        Some(SettingsAction::Diff { domain, since }) => {
            render::diff_value(backend, domain, *since)
        }
        Some(SettingsAction::BulkGet { domain, keys }) => {
            let names: Vec<&str> = keys.iter().map(String::as_str).collect();
            render::bulk_get_value(backend, domain, &names)
        }
        Some(SettingsAction::BulkSet { domain, entries }) => {
            guard_host_write(guard, &format!("settings bulk-set {domain}"))?;
            let mut typed = Vec::new();
            for entry in entries {
                let (key, value) = entry.split_once('=').ok_or_else(|| {
                    SmartError::invalid_argument(format!(
                        "a bulk entry is key=value, got {entry:?}"
                    ))
                })?;
                typed.push(smart_lib::domains::settings::types::SettingMapEntry {
                    key: key.to_string(),
                    value: render::typed_value(backend, domain, key, value)?,
                });
            }
            backend.bulk_set(domain, &typed).map_err(SmartError::from)?;
            Ok(render::verb_value(
                "bulk-set",
                &format!("{domain} ({} entries)", typed.len()),
            ))
        }
        Some(SettingsAction::Watch {
            domain,
            key,
            count,
            interval,
        }) => render::watch_value(backend, domain, key.as_deref(), *count, *interval),
        Some(SettingsAction::Caps) => Ok(render::caps_value(backend)),
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(cmd: &SettingsCmd, format: &OutputFormat, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
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

/// The settings selector tree, rendered from the engine's owned types — the
/// same values the command surface renders, so a selector and a command can
/// never disagree. `@settings/domains`, `@settings/caps`, `@settings/schema`
/// resolve from here.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    Ok(Value::Object(vec![
        ("domains".into(), render::domains_value(backend)?),
        ("caps".into(), render::caps_value(backend)),
        (
            "schemas".into(),
            render::schemas_value(backend, ""),
        ),
    ]))
}

/// One `SettingValue`'s own words, through the engine's own renderer (the
/// contract's value model, printed as the host spells it).
pub(crate) fn value_words(value: &SettingValue) -> String {
    render_value(value)
}

/// One `SchemaInfo` as a record.
pub(crate) fn schema_value_of(schema: &SchemaInfo) -> Value {
    Value::Object(vec![
        ("id".into(), Value::String(schema.id.clone())),
        (
            "path".into(),
            match &schema.path {
                Some(path) => Value::String(path.clone()),
                None => Value::Null,
            },
        ),
        ("relocatable".into(), Value::Bool(schema.relocatable)),
        ("keys".into(), Value::Number(schema.keys.len() as f64)),
    ])
}

/// One `SchemaKey` as a record: the host's own declaration.
pub(crate) fn key_value_of(key: &SchemaKey) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(key.name.clone())),
        ("type".into(), Value::String(key.variant_type.clone())),
        (
            "contract_type".into(),
            match key.contract_value() {
                Some(token) => Value::String(token.to_string()),
                None => Value::Null,
            },
        ),
        (
            "default".into(),
            match &key.default_value {
                Some(default) => Value::String(default.clone()),
                None => Value::Null,
            },
        ),
        (
            "range".into(),
            match &key.range {
                Some(range) => Value::String(range.clone()),
                None => Value::Null,
            },
        ),
        (
            "summary".into(),
            match &key.summary {
                Some(summary) => Value::String(summary.clone()),
                None => Value::Null,
            },
        ),
    ])
}

/// One `SettingChange` as a record (the contract's own audit shape).
pub(crate) fn change_value(change: &SettingChange) -> Value {
    Value::Object(vec![
        ("domain".into(), Value::String(change.domain.clone())),
        ("key".into(), Value::String(change.key.clone())),
        ("value".into(), Value::String(value_words(&change.value))),
        (
            "timestamp".into(),
            Value::String(change.timestamp.as_nanos().to_string()),
        ),
        ("actor".into(), Value::String(change.actor_id.clone())),
        ("version".into(), Value::Number(change.version as f64)),
    ])
}

/// The `get` union's own words: the value, the notSet answer, or the host's
/// error — never a fabricated value.
pub(crate) fn result_words(result: &SettingResult) -> Value {
    match result {
        SettingResult::Value(value) => Value::Object(vec![
            ("set".into(), Value::Bool(true)),
            ("value".into(), Value::String(value_words(value))),
            ("type".into(), Value::String(value.type_token().to_string())),
        ]),
        SettingResult::NotSet => Value::Object(vec![
            ("set".into(), Value::Bool(false)),
            (
                "reason".into(),
                Value::String(
                    "the host's own store holds no value for this key — the schema's default \
                     (shown beside it) is the host's answer, not a value it carries"
                        .to_string(),
                ),
            ),
        ]),
        SettingResult::Error(e) => Value::Object(vec![
            ("error".into(), Value::String(e.to_string())),
        ]),
        SettingResult::Unknown => Value::Object(vec![
            (
                "error".into(),
                Value::String("get answered the contract's unknown union arm".to_string()),
            ),
        ]),
    }
}

/// Re-export for the shell and main: the clap shape.
pub use self::{SettingsAction as Action, SettingsCmd as Cmd};

/// The engine's published reason for one contract method, so a degraded row is
/// answered with the matrix's own measured words.
pub(crate) fn matrix_reason(method: &str) -> Option<String> {
    use smart_lib::domains::settings::availability::{method_availability_for, AvailStatus};
    let row = method_availability_for(method)?;
    let word = match row.status {
        AvailStatus::Real => "REAL",
        AvailStatus::Degraded => "DEGRADED",
        AvailStatus::Unavailable => "UNAVAILABLE",
    };
    Some(if row.reason.is_empty() {
        word.to_string()
    } else {
        format!("{word} — {}", row.reason)
    })
}
