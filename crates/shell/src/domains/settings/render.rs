//! The settings domain's rendering, in the shell's aesthetic: box-drawing
//! tables, `◉/○`, the host's own schema words. Every renderer reads the
//! ENGINE'S OWNED TYPES (`SchemaInfo`, `SchemaKey`, `SettingValue`,
//! `SettingResult`, `SettingChange`) — there is no capnp reader here, so a
//! schema key the host publishes and a value the host carries are printed
//! exactly as the engine read them.
//!
//! Every width is a display width under the terminal-faithful measure, and
//! every cell is CONTENT plus a STYLE (`Cell`), never a pre-styled string.

use crate::color::ColorExt;
use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::widgets::{truncate_to, Cell, Sgr, Table};
use crate::OutputFormat;

use smart_lib::domains::settings::SettingsBackend;
use smart_lib::domains::settings::types::SettingValue;

use super::{
    change_value, key_value_of, matrix_reason, result_words, schema_value_of, value_words,
};
use super::{SettingsAction as A, SettingsCmd};

// ═══ the data half ═══════════════════════════════════════════════════════

/// `settings` — the host's own schema source: every published schema with its
/// own key count, path and relocatability.
pub fn domains_value(backend: &dyn SettingsBackend) -> SmartResult<Value> {
    let domains = backend.list_domains(super::all()).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "domains".into(),
            Value::List(
                domains
                    .items
                    .iter()
                    .map(|domain| {
                        let schema = backend.describe(domain).ok().flatten();
                        Value::Object(vec![
                            ("id".into(), Value::String(domain.clone())),
                            (
                                "keys".into(),
                                Value::Number(schema.as_ref().map(|s| s.keys.len()).unwrap_or(0) as f64),
                            ),
                            (
                                "path".into(),
                                match schema.as_ref().and_then(|s| s.path.clone()) {
                                    Some(path) => Value::String(path),
                                    None => Value::Null,
                                },
                            ),
                            (
                                "relocatable".into(),
                                Value::Bool(schema.as_ref().map(|s| s.relocatable).unwrap_or(false)),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(domains.total as f64)),
        (
            "reason".into(),
            Value::String(
                "the host's own compiled schema source (gio's SettingsSchemaSource), read natively \
                 — the same table `gsettings list-schemas` is a CLI over"
                    .to_string(),
            ),
        ),
    ]))
}

/// `settings list <domain>` — the domain's live values, the contract's own
/// StringMap, with the schema's own default beside any key the host never set.
pub fn list_value(backend: &dyn SettingsBackend, domain: &str) -> SmartResult<Value> {
    let map = backend.get_all(domain, super::all()).map_err(SmartError::from)?;
    let schema = backend.describe(domain).ok().flatten();
    Ok(Value::Object(vec![
        ("domain".into(), Value::String(domain.to_string())),
        (
            "settings".into(),
            Value::List(
                map.entries
                    .iter()
                    .map(|entry| {
                        // The schema's own default travels beside the value, so
                        // a caller can tell a value the host carries from a
                        // default it merely publishes.
                        let default = schema
                            .as_ref()
                            .and_then(|s| s.key(&entry.key))
                            .and_then(|key| key.default_value.clone());
                        Value::Object(vec![
                            ("key".into(), Value::String(entry.key.clone())),
                            ("value".into(), Value::String(value_words(&entry.value))),
                            ("type".into(), Value::String(entry.value.type_token().to_string())),
                            (
                                "default".into(),
                                match default {
                                    Some(default) => Value::String(default),
                                    None => Value::Null,
                                },
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(map.entries.len() as f64)),
    ]))
}

/// `settings get` — the contract's own union, so a key the host never set is an
/// ANSWER (with the schema's own default named beside it).
pub fn get_value(backend: &dyn SettingsBackend, domain: &str, key: &str) -> SmartResult<Value> {
    let result = backend.get(domain, key).map_err(SmartError::from)?;
    let mut out = match result_words(&result) {
        Value::Object(fields) => fields,
        _ => Vec::new(),
    };
    out.insert(0, ("domain".into(), Value::String(domain.to_string())));
    out.insert(1, ("key".into(), Value::String(key.to_string())));
    // The schema's own declaration for this key: type, default, range.
    if let Some(schema) = backend.describe(domain).ok().flatten() {
        if let Some(declared) = schema.key(key) {
            out.push(("schema_type".into(), Value::String(declared.variant_type.clone())));
            out.push((
                "schema_default".into(),
                match &declared.default_value {
                    Some(default) => Value::String(default.clone()),
                    None => Value::Null,
                },
            ));
            out.push((
                "schema_range".into(),
                match &declared.range {
                    Some(range) => Value::String(range.clone()),
                    None => Value::Null,
                },
            ));
            out.push((
                "summary".into(),
                match &declared.summary {
                    Some(summary) => Value::String(summary.clone()),
                    None => Value::Null,
                },
            ));
        }
    }
    Ok(Value::Object(out))
}

/// The value a gated write hands the engine, read through the schema's own
/// type when the host publishes one — so a `b` key is a boolean and an `i` key
/// is an integer, never a coerced string.
pub fn typed_value(
    backend: &dyn SettingsBackend,
    domain: &str,
    key: &str,
    raw: &str,
) -> SmartResult<SettingValue> {
    let variant = backend
        .describe(domain)
        .ok()
        .flatten()
        .and_then(|schema| schema.key(key).map(|key| key.variant_type.clone()))
        .unwrap_or_default();
    Ok(match variant.as_str() {
        "b" | "mb" => SettingValue::Boolean(matches!(raw, "true" | "1" | "yes" | "on")),
        "i" | "u" | "x" | "q" | "h" | "y" | "n" | "t" => match raw.parse::<i64>() {
            Ok(number) => SettingValue::Integer(number),
            Err(_) => SettingValue::String(raw.to_string()),
        },
        "d" => match raw.parse::<f64>() {
            Ok(number) => SettingValue::Real(number),
            Err(_) => SettingValue::String(raw.to_string()),
        },
        _ => SettingValue::String(raw.to_string()),
    })
}

/// `settings schema <domain>` — the schema browser: the host's own declaration
/// for every key, read out of the compiled schema source.
pub fn schema_value(backend: &dyn SettingsBackend, domain: &str) -> SmartResult<Value> {
    match backend.describe(domain).map_err(SmartError::from)? {
        Some(schema) => {
            let keys: Vec<Value> = schema.keys.iter().map(key_value_of).collect();
            Ok(Value::Object(vec![
                ("domain".into(), Value::String(schema.id.clone())),
                (
                    "path".into(),
                    match &schema.path {
                        Some(path) => Value::String(path.clone()),
                        None => Value::Null,
                    },
                ),
                ("relocatable".into(), Value::Bool(schema.relocatable)),
                ("keys".into(), Value::List(keys)),
                ("total".into(), Value::Number(schema.keys.len() as f64)),
            ]))
        }
        // A schema the host does not publish is notFound, not an abort and not
        // an invented schema.
        None => Err(SmartError::NotFound(format!(
            "this host's own schema source publishes no schema named {domain}"
        ))),
    }
}

/// The whole schema source as records (the selector tree's `schemas`).
pub fn schemas_value(backend: &dyn SettingsBackend, prefix: &str) -> Value {
    match backend.describe_all() {
        Ok(schemas) => Value::List(
            schemas
                .iter()
                .filter(|schema| schema.id.starts_with(prefix))
                .map(schema_value_of)
                .collect(),
        ),
        Err(_) => Value::List(Vec::new()),
    }
}

/// `settings category` — the contract's own OPEN category token, recorded
/// beside each key rather than guessed (glib publishes no per-key category).
pub fn category_value(backend: &dyn SettingsBackend, category: &str) -> SmartResult<Value> {
    let page = backend
        .list_by_category(category, super::all())
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("category".into(), Value::String(category.to_string())),
        (
            "entries".into(),
            Value::List(
                page.items
                    .iter()
                    .map(|entry| {
                        Value::Object(vec![
                            ("key".into(), Value::String(entry.key.clone())),
                            ("value".into(), Value::String(value_words(&entry.value))),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(page.total as f64)),
        (
            "reason".into(),
            Value::String(
                matrix_reason("listByCategory")
                    .unwrap_or_else(|| "the contract's category tokens are conventions".to_string()),
            ),
        ),
    ]))
}

/// One key's recorded history.
pub fn history_value(backend: &dyn SettingsBackend, domain: &str, key: &str) -> SmartResult<Value> {
    let page = backend
        .get_history(domain, key, super::all())
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("domain".into(), Value::String(domain.to_string())),
        ("key".into(), Value::String(key.to_string())),
        (
            "changes".into(),
            Value::List(page.items.iter().map(change_value).collect()),
        ),
        ("total".into(), Value::Number(page.total as f64)),
    ]))
}

/// The delta read: everything after one version, plus the version now.
pub fn diff_value(backend: &dyn SettingsBackend, domain: &str, since: u64) -> SmartResult<Value> {
    let (changes, version) = backend
        .get_settings_diff(domain, since)
        .map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("domain".into(), Value::String(domain.to_string())),
        ("since_version".into(), Value::Number(since as f64)),
        ("version".into(), Value::Number(version as f64)),
        (
            "changes".into(),
            Value::List(changes.iter().map(change_value).collect()),
        ),
    ]))
}

/// The StringMap read: several keys at once.
pub fn bulk_get_value(backend: &dyn SettingsBackend, domain: &str, keys: &[&str]) -> SmartResult<Value> {
    let entries = backend.bulk_get(domain, keys).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("domain".into(), Value::String(domain.to_string())),
        (
            "settings".into(),
            Value::List(
                entries
                    .iter()
                    .map(|entry| {
                        Value::Object(vec![
                            ("key".into(), Value::String(entry.key.clone())),
                            ("value".into(), Value::String(value_words(&entry.value))),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("total".into(), Value::Number(entries.len() as f64)),
    ]))
}

/// The host's own change signal, delivered live: one subscription, bounded
/// pulls, the changes the host reported.
pub fn watch_value(
    backend: &dyn SettingsBackend,
    domain: &str,
    key: Option<&str>,
    count: u32,
    interval_ms: u64,
) -> SmartResult<Value> {
    let subscription = match key {
        Some(key) => backend.watch(domain, key),
        None => backend.watch_domain(domain),
    }
    .map_err(SmartError::from)?;
    let mut changes = Vec::new();
    for _ in 0..count.max(1) {
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        let batch = backend
            .pull_settings(subscription, 16)
            .map_err(SmartError::from)?;
        for change in batch {
            changes.push(change_value(&change));
        }
    }
    let _ = backend.cancel_settings(subscription);
    Ok(Value::Object(vec![
        ("domain".into(), Value::String(domain.to_string())),
        (
            "key".into(),
            match key {
                Some(key) => Value::String(key.to_string()),
                None => Value::Null,
            },
        ),
        ("changes".into(), Value::List(changes.clone())),
        ("total".into(), Value::Number(changes.len() as f64)),
        (
            "reason".into(),
            Value::String(
                "the host's own `changed` signal on its own settings object, delivered \
                 synchronously — the same signal `gsettings monitor` watches, with no fork"
                    .to_string(),
            ),
        ),
    ]))
}

/// This backend's honest capability tokens (law #6).
pub fn caps_value(backend: &dyn SettingsBackend) -> Value {
    Value::List(
        backend
            .settings_caps()
            .into_iter()
            .map(|token| Value::String(token.to_string()))
            .collect(),
    )
}

/// A gated write's answer once the gate accepted it.
pub fn verb_value(verb: &str, target: &str) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("target".into(), Value::String(target.to_string())),
        ("changed".into(), Value::Bool(true)),
    ])
}

// ═══ the aesthetic ════════════════════════════════════════════════════════

/// Render one command's value in the requested format.
pub fn render_action(cmd: &SettingsCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &SettingsCmd, value: &Value) -> String {
    match &cmd.action {
        None | Some(A::Domains) => render_domains(value),
        Some(A::List { domain }) => render_list(value, domain),
        Some(A::Get { domain, key }) => render_get(value, domain, key),
        Some(A::Set { .. }) => render_verb(value),
        Some(A::Schema { domain }) => render_schema(value, domain),
        Some(A::Category { category }) => render_category(value, category),
        Some(A::Version { domain, key }) => render_version(value, domain, key),
        Some(A::History { domain, key }) => render_history(value, domain, key),
        Some(A::Diff { domain, .. }) => render_diff(value, domain),
        Some(A::BulkGet { domain, .. }) => render_bulk_get(value, domain),
        Some(A::BulkSet { .. }) => render_verb(value),
        Some(A::Watch { domain, .. }) => render_watch(value, domain),
        Some(A::Caps) => render_caps(value),
    }
}

/// The schema source: every domain the host publishes. The row count is the
/// caller's (`| take 24`, `--limit`) — the 24-of-258 peephole that used to
/// live here hid the host's own source; the width is the renderer's, and the
/// full 258 are one door away in `-o json`.
fn render_domains(value: &Value) -> String {
    let mut out = String::new();
    let domains = match value.field("domains") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let total = value.field("total").as_f64().unwrap_or(0.0) as usize;
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "⚙".blue().bold(),
        "Settings".blue().bold(),
        format!("({total})").dimmed(),
        "— the host's own schema source, read natively".dimmed()
    ));
    if domains.is_empty() {
        out.push_str(&format!(
            "  └─ {} the host publishes no schema (an honest empty — the source was read)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec!["".into(), "DOMAIN".into(), "KEYS".into(), "PATH".into()])
        .cap(1, 52)
        .cap(3, 44);
    for domain in &domains {
        table = table.row(vec![
            yes_no(!domain.field_bool("relocatable")),
            domain.field_str("id").into(),
            Cell::plain(domain.field_str("keys")),
            match domain.field("path") {
                Value::String(path) => Cell::path(path.clone()),
                _ => Cell::plain("—"),
            },
        ]);
    }
    out.push_str(&table.render());
    out.push_str(&format!(
        "\n  {} the full list: {} · one schema: {}\n",
        "●".blue().bold(),
        "smart settings -o json".dimmed(),
        "smart settings schema <domain>".dimmed()
    ));
    out.trim_end().to_string()
}

/// One domain's live values, with the schema's own default beside each key.
fn render_list(value: &Value, domain: &str) -> String {
    let settings = match value.field("settings") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} — the host's own dconf values\n",
        "🗂".blue().bold(),
        "Settings".blue().bold(),
        domain.bold(),
        format!("({})", settings.len()).dimmed()
    ));
    if settings.is_empty() {
        out.push_str(&format!(
            "  └─ {} the schema publishes no key (an honest empty — the host was read)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "KEY".into(),
        "VALUE".into(),
        "TYPE".into(),
        "SCHEMA DEFAULT".into(),
    ]);
    for setting in &settings {
        table = table.row(vec![
            Cell::plain(setting.field_str("key")),
            truncate_to(&setting.field_str("value"), 40).into(),
            Cell::plain(setting.field_str("type")),
            match setting.field("default") {
                Value::String(default) => truncate_to(default, 28).into(),
                _ => Cell::plain("—"),
            },
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

/// One key's read: the contract's own union, with the schema's own words.
/// The schema's GVariant type token as the word an operator reads. The token is
/// the schema source's own spelling; the word is what the value IS. Both stay
/// reachable — the word here, the token under `-o json`.
fn type_word(token: &str) -> String {
    match token {
        "b" | "mb" => "boolean".to_string(),
        "y" | "n" | "q" | "i" | "u" | "x" | "h" | "t" => "integer".to_string(),
        "d" => "real".to_string(),
        "s" | "o" | "g" => "string".to_string(),
        other if other.starts_with('a') => "list".to_string(),
        other if other.starts_with('{') => "dict".to_string(),
        other => other.to_string(),
    }
}

fn render_get(value: &Value, domain: &str, key: &str) -> String {
    let mut out = String::new();
    // The header's type word is the HUMAN one (boolean/integer/string/…), never
    // the raw GVariant token the schema source spells (`(as)`, `(i)`, `(mb)`).
    // The raw token still rides `-o json`, where a machine can use it.
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "🔎".blue().bold(),
        "Setting".blue().bold(),
        format!("{domain} {key}").bold(),
        format!("({})", type_word(&value.field_str("schema_type"))).dimmed()
    ));
    if !value.field_str("error").is_empty() {
        out.push_str(&format!("  └─ {} {}\n", "✗".red(), value.field_str("error")));
        return out.trim_end().to_string();
    }
    if value.field_bool("set") {
        out.push_str(&format!(
            "  ├─ Value       {} {}\n",
            value.field_str("value").bold(),
            format!("({})", value.field_str("type")).dimmed()
        ));
    } else {
        // `notSet` is an answer: the schema's own default is named as the
        // default, never painted as the value.
        out.push_str(&format!(
            "  ├─ Value       {} the host carries no value for this key\n",
            "○".dimmed()
        ));
        let default = value.field_str("schema_default");
        out.push_str(&format!(
            "  ├─ Default     {}\n",
            if default.is_empty() {
                "— the schema publishes none".to_string()
            } else {
                format!("{default} (the schema's own default)")
            }
        ));
    }
    let range = value.field_str("schema_range");
    if !range.is_empty() {
        out.push_str(&format!("  ├─ Range       {}\n", range));
    }
    let summary = value.field_str("summary");
    if !summary.is_empty() {
        out.push_str(&format!("  ├─ Summary     {}\n", truncate_to(&summary, 64)));
    }
    out.push_str("  └─ Source     the host's own schema source + its own dconf database");
    out.trim_end().to_string()
}

/// The schema browser: the host's own declaration for every key.
fn render_schema(value: &Value, domain: &str) -> String {
    let keys = match value.field("keys") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} {}\n",
        "📚".blue().bold(),
        "Schema".blue().bold(),
        domain.bold(),
        format!("({} keys)", keys.len()).dimmed(),
        match value.field("path") {
            Value::String(path) => format!("— {path}").dimmed(),
            _ => "— relocatable".to_string().dimmed(),
        }
    ));
    if keys.is_empty() {
        out.push_str(&format!(
            "  └─ {} the schema publishes no key (an honest empty)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "KEY".into(),
        "TYPE".into(),
        "CONTRACT".into(),
        "DEFAULT".into(),
        "RANGE".into(),
        "SUMMARY".into(),
    ]);
    for key in &keys {
        table = table.row(vec![
            Cell::plain(truncate_to(&key.field_str("name"), 28)),
            Cell::plain(key.field_str("type")),
            match key.field("contract_type") {
                Value::String(token) => Cell::plain(token),
                _ => Cell::styled("—", Sgr::Dimmed),
            },
            match key.field("default") {
                Value::String(default) => truncate_to(default, 20).into(),
                _ => Cell::plain("—"),
            },
            match key.field("range") {
                Value::String(range) => truncate_to(range, 20).into(),
                _ => Cell::plain("—"),
            },
            match key.field("summary") {
                Value::String(summary) => truncate_to(summary, 48).into(),
                _ => Cell::plain("—"),
            },
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

fn render_category(value: &Value, category: &str) -> String {
    let entries = match value.field("entries") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} — the contract's own OPEN category token\n",
        "🏷".blue().bold(),
        "Category".blue().bold(),
        format!("({})", entries.len()).dimmed(),
        category.bold()
    ));
    out.push_str(&format!("  └─ {}\n", value.field_str("reason")));
    if entries.is_empty() {
        out.push_str(&format!(
            "  {} no key is recorded under {category} — the category is recorded beside each key, \
             never guessed\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec!["KEY".into(), "VALUE".into()]);
    for entry in &entries {
        table = table.row(vec![
            Cell::plain(entry.field_str("key")),
            truncate_to(&entry.field_str("value"), 40).into(),
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

fn render_version(value: &Value, domain: &str, key: &str) -> String {
    format!(
        "  {} {} {} {} → version {}\n",
        "🔢".blue().bold(),
        "Version".blue().bold(),
        domain,
        key,
        value.field_str("version").bold()
    )
    .trim_end()
    .to_string()
}

fn render_history(value: &Value, domain: &str, key: &str) -> String {
    let changes = match value.field("changes") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} {}\n",
        "📜".blue().bold(),
        "History".blue().bold(),
        format!("{domain} {key}").bold(),
        format!("({})", changes.len()).dimmed(),
        "— this backend's own monotonic store".dimmed()
    ));
    if changes.is_empty() {
        out.push_str(&format!(
            "  └─ {} the store records no change for this key\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "VERSION".into(),
        "ACTOR".into(),
        "VALUE".into(),
        "WHEN".into(),
    ]);
    for change in &changes {
        table = table.row(vec![
            Cell::plain(change.field_str("version")),
            Cell::plain(change.field_str("actor")),
            truncate_to(&change.field_str("value"), 36).into(),
            Cell::plain(change.field_str("timestamp")),
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

fn render_diff(value: &Value, domain: &str) -> String {
    let changes = match value.field("changes") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} {}\n",
        "⇄".blue().bold(),
        "Diff".blue().bold(),
        domain.bold(),
        format!("({})", changes.len()).dimmed(),
        format!("since version {} → {}", value.field_str("since_version"), value.field_str("version")).dimmed()
    ));
    if changes.is_empty() {
        out.push_str(&format!(
            "  └─ {} nothing changed after that version (a measured empty)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec!["KEY".into(), "VALUE".into(), "VERSION".into()]);
    for change in &changes {
        table = table.row(vec![
            Cell::plain(change.field_str("key")),
            truncate_to(&change.field_str("value"), 40).into(),
            Cell::plain(change.field_str("version")),
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

fn render_bulk_get(value: &Value, domain: &str) -> String {
    let settings = match value.field("settings") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} — the contract's StringMap read\n",
        "📥".blue().bold(),
        "Bulk read".blue().bold(),
        domain.bold(),
        format!("({})", settings.len()).dimmed()
    ));
    let mut table = Table::new(vec!["KEY".into(), "VALUE".into()]);
    for setting in &settings {
        table = table.row(vec![
            Cell::plain(setting.field_str("key")),
            truncate_to(&setting.field_str("value"), 48).into(),
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

/// The host's own change signal, delivered live.
fn render_watch(value: &Value, domain: &str) -> String {
    let changes = match value.field("changes") {
        Value::List(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} {} {} — the host's own `changed` signal\n",
        "👁".blue().bold(),
        "Watch".blue().bold(),
        domain.bold(),
        format!("({} event(s))", changes.len()).dimmed()
    ));
    if changes.is_empty() {
        out.push_str(&format!(
            "  └─ {} no change arrived in the bounded rounds (the subscription is real — the host \\
             simply reported nothing)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec!["".into(), "KEY".into(), "VALUE".into(), "VERSION".into()]);
    for change in &changes {
        table = table.row(vec![
            Cell::styled("∼", Sgr::Green),
            Cell::plain(change.field_str("key")),
            truncate_to(&change.field_str("value"), 40).into(),
            Cell::plain(change.field_str("version")),
        ]);
    }
    out.push_str(&table.render());
    out.push_str(&format!("\n  {}\n", value.field_str("reason")));
    out.trim_end().to_string()
}

fn render_caps(value: &Value) -> String {
    let Value::List(caps) = value else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — what this backend serves\n",
        "🔑".blue().bold(),
        "Settings caps".blue().bold()
    ));
    for (index, cap) in caps.iter().enumerate() {
        let branch = if index + 1 == caps.len() { "└─" } else { "├─" };
        out.push_str(&format!("  {} {} {}\n", branch, "◉".green(), cap.display()));
    }
    out.trim_end().to_string()
}

/// A gated write's answer once the gate accepted it.
fn render_verb(value: &Value) -> String {
    format!(
        "  {} {} {} {}",
        "✎".green(),
        value.field_str("verb").bold(),
        value.field_str("target"),
        "— the engine's own host-mutating path, accepted".dimmed()
    )
    .trim_end()
    .to_string()
}

fn yes_no(yes: bool) -> Cell {
    if yes {
        Cell::styled("◉", Sgr::Green)
    } else {
        Cell::styled("○", Sgr::Dimmed)
    }
}
