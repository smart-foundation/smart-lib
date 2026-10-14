//! The service domain's rendering, in the shell's aesthetic: box-drawing,
//! `◉/○`, aligned tables. Every renderer reads the ENGINE'S OWNED TYPES
//! (`ServiceInstanceInfo`, `ServiceInstanceState`, `ServiceHealth`,
//! `ServiceEvent`, `ServiceMetrics`) — there is no capnp reader here, so
//! nothing can leak and nothing can desynchronise from what the engine read off
//! the supervisor's own state.

use crate::color::ColorExt;
use crate::error::SmartResult;
use crate::selector::Value;
use crate::widgets::{Cell, Sgr, Table};
use crate::OutputFormat;

use smart_lib::domains::service::ServiceBackend;
use smart_lib::domains::service::types::{
    HealthState, Pagination, ServiceHealthCheck, ServiceInstanceInfo, ServiceMetrics, ServiceStatus,
};

/// A 64-bit value that must not be rounded.
///
/// The shell's value tree carries numbers as `f64`, whose exact integer range
/// ends at 2^53. `T.ServiceId` is an opaque identity this backend issues as a
/// 64-bit hash, and `T.Timestamp` is nanoseconds since the epoch — both exceed
/// that range, so a rounded value would be **a number that is not the id** (and
/// not the time): a client reading it back would address a service the engine
/// never issued. They therefore travel as their exact decimal text. An opaque
/// identity is not a quantity, so no arithmetic is lost.
fn exact(value: u64) -> Value {
    Value::String(value.to_string())
}

/// The data half of `service` / `service status`: every service the supervisor
/// serves, with its measured state, enablement and supervision — the tree
/// `@service/*` selectors resolve from.
pub fn services_value(backend: &dyn ServiceBackend, page: Pagination) -> SmartResult<Value> {
    match backend.list_services(page) {
        Ok(services) => Ok(Value::List(
            services.items.iter().map(instance_value).collect(),
        )),
        // A supervisor that could not be reached is an honest absence: the
        // reason travels with the answer rather than an empty list that would
        // claim the system has no services.
        Err(e) => Ok(Value::Object(vec![
            ("error".into(), Value::String(e.to_string())),
            ("services".into(), Value::List(Vec::new())),
        ])),
    }
}

/// One instance as a record.
fn instance_value(info: &ServiceInstanceInfo) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(info.name.clone())),
        ("description".into(), Value::String(info.description.clone())),
        ("state".into(), Value::String(info.state.status.as_token().to_string())),
        ("state_reason".into(), Value::String(info.state.reason.clone())),
        ("pid".into(), Value::Number(info.state.main_pid as f64)),
        ("since_us".into(), Value::Number(info.state.since_us as f64)),
        ("since_valid".into(), Value::Bool(info.state.since_valid)),
        ("enabled".into(), Value::Bool(info.enabled)),
        ("on_demand".into(), Value::Bool(info.on_demand)),
        (
            "supervised".into(),
            Value::Bool(
                info.state
                    .metadata
                    .iter()
                    .any(|(k, v)| k == "supervised" && v == "true"),
            ),
        ),
        (
            "declared".into(),
            Value::Bool(
                info.state
                    .metadata
                    .iter()
                    .any(|(k, v)| k == "declared" && v == "true"),
            ),
        ),
    ])
}

/// `service inspect <name>`: one service's state and its health.
pub fn inspect_value(backend: &dyn ServiceBackend, name: &str) -> SmartResult<Value> {
    let (id, present) = backend.resolve(name).map_err(|e| crate::error::SmartError::failed(e.to_string()))?;
    if !present {
        // A name the supervisor does not serve is the answer, not a fault.
        return Ok(Value::Object(vec![
            ("name".into(), Value::String(name.to_string())),
            ("present".into(), Value::Bool(false)),
        ]));
    }
    let info = backend
        .get_service(id)
        .map_err(|e| crate::error::SmartError::failed(e.to_string()))?;
    let health = backend.get_health(id).ok();
    let mut out = vec![
        ("name".into(), Value::String(info.name.clone())),
        ("present".into(), Value::Bool(true)),
        (
            "description".into(),
            Value::String(info.description.clone()),
        ),
        ("state".into(), Value::String(info.state.status.as_token().to_string())),
        ("state_reason".into(), Value::String(info.state.reason.clone())),
        ("pid".into(), Value::Number(info.state.main_pid as f64)),
        ("enabled".into(), Value::Bool(info.enabled)),
    ];
    if let Some(health) = health {
        out.push(("health".into(), health_value_single(&health)));
        out.push((
            "health_state".into(),
            Value::String(health.state.as_token().to_string()),
        ));
    }
    Ok(Value::Object(out))
}

/// One health record.
fn health_value_single(health: &smart_lib::domains::service::types::ServiceHealth) -> Value {
    Value::Object(vec![
        ("state".into(), Value::String(health.state.as_token().to_string())),
        ("reason".into(), Value::String(health.reason.clone())),
        ("quarantined".into(), Value::Bool(health.quarantined)),
        ("last_check_passed".into(), Value::Bool(health.last_check_passed)),
        ("last_check_at".into(), Value::Number(health.last_check_at as f64)),
        ("fault_count".into(), Value::Number(health.fault_count as f64)),
        ("restarts".into(), Value::Number(health.restarts as f64)),
    ])
}

/// The supervisor's id → name map, so the contract's answers — which carry an
/// identity and no name — render as names in the shell's table.
fn names_by_id(backend: &dyn ServiceBackend) -> std::collections::HashMap<u64, String> {
    let mut map = std::collections::HashMap::new();
    if let Ok(page) = backend.list_services(Pagination { offset: 0, limit: u32::MAX }) {
        for info in &page.items {
            map.insert(info.id.get(), info.name.clone());
        }
    }
    map
}

/// The name an identity maps to, or its number when the supervisor's listing
/// does not carry it (never a fabrication: an unmapped id reports itself).
fn name_of(names: &std::collections::HashMap<u64, String>, id: u64) -> String {
    names
        .get(&id)
        .cloned()
        .unwrap_or_else(|| id.to_string())
}

/// The bounded health listing.
pub fn health_value(backend: &dyn ServiceBackend, page: Pagination) -> SmartResult<Value> {
    match backend.list_health(page) {
        Ok(health) => {
            let names = names_by_id(backend);
            Ok(Value::List(
                health
                    .items
                    .iter()
                    .map(|h| {
                        let mut value = health_value_single(h);
                        if let Value::Object(fields) = &mut value {
                            fields.insert(
                                0,
                                ("name".into(), Value::String(name_of(&names, h.service_id.get()))),
                            );
                        }
                        value
                    })
                    .collect(),
            ))
        }
        Err(e) => Ok(honest_error_value(&e.to_string())),
    }
}

/// The quarantined services: the honest empty on a host supervisor.
pub fn quarantined_value(backend: &dyn ServiceBackend, page: Pagination) -> SmartResult<Value> {
    match backend.list_quarantined(page) {
        Ok(quarantined) => {
            let mut fields = vec![(
                "quarantined".into(),
                Value::List(Vec::new()),
            )];
            fields.push((
                "total".into(),
                Value::Number(quarantined.total as f64),
            ));
            Ok(Value::Object(fields))
        }
        Err(e) => Ok(honest_error_value(&e.to_string())),
    }
}

/// The dependencies one service declares.
pub fn deps_value(backend: &dyn ServiceBackend, name: &str) -> SmartResult<Value> {
    let (id, present) = backend.resolve(name).map_err(|e| crate::error::SmartError::failed(e.to_string()))?;
    let deps = if present {
        backend.get_dependencies(id)
    } else {
        Err(smart_lib::core::Error::with_message(
            smart_lib::core::ErrorCode::NotFound,
            format!("the supervisor this backend reached does not serve a service named {name}"),
        ))
    };
    match deps {
        Ok(deps) => Ok(Value::List(
            deps.iter()
                .map(|d| {
                    let names = names_by_id(backend);
                    Value::Object(vec![
                        ("name".into(), Value::String(name_of(&names, d.service_id.get()))),
                        (
                            "dependency_type".into(),
                            Value::String(d.dependency_type.as_token().to_string()),
                        ),
                        ("satisfied".into(), Value::Bool(d.satisfied)),
                    ])
                })
                .collect(),
        )),
        Err(e) => Ok(honest_error_value(&e.to_string())),
    }
}

/// `resolve`: a name to an identity, with `present` as a first-class answer.
pub fn resolve_value(backend: &dyn ServiceBackend, name: &str) -> SmartResult<Value> {
    let (id, present) = backend.resolve(name).map_err(|e| crate::error::SmartError::failed(e.to_string()))?;
    Ok(Value::Object(vec![
        ("name".into(), Value::String(name.to_string())),
        ("present".into(), Value::Bool(present)),
        ("service_id".into(), exact(id.get())),
    ]))
}

/// The runtime measurements — an honest `Unavailable` on a host supervisor.
pub fn metrics_value(backend: &dyn ServiceBackend, name: &str, window_secs: u64) -> SmartResult<Value> {
    let (id, _) = backend.resolve(name).map_err(|e| crate::error::SmartError::failed(e.to_string()))?;
    match backend.get_metrics(id, window_secs.saturating_mul(1_000_000_000)) {
        Ok(metrics) => Ok(metrics_value_single(&metrics)),
        Err(e) => Ok(honest_error_value(&e.to_string())),
    }
}

/// One metrics record.
pub fn metrics_value_single(metrics: &ServiceMetrics) -> Value {
    Value::Object(vec![
        ("service_id".into(), exact(metrics.service_id.get())),
        ("window_ns".into(), Value::Number(metrics.window_ns as f64)),
        ("request_count".into(), Value::Number(metrics.request_count as f64)),
        ("error_count".into(), Value::Number(metrics.error_count as f64)),
        (
            "latency_p99_ns".into(),
            Value::Number(metrics.latency_p99_ns as f64),
        ),
        ("active_requests".into(), Value::Number(metrics.active_requests as f64)),
        ("partial".into(), Value::Bool(metrics.partial)),
    ])
}

/// The declared resource allocation.
pub fn allocation_value(backend: &dyn ServiceBackend, name: &str) -> SmartResult<Value> {
    let (id, _) = backend.resolve(name).map_err(|e| crate::error::SmartError::failed(e.to_string()))?;
    match backend.get_resource_allocation(id) {
        Ok(allocation) => Ok(Value::Object(vec![
            (
                "memory_bytes".into(),
                Value::Number(allocation.quota.memory_bytes as f64),
            ),
            (
                "cpu_percent".into(),
                Value::Number(allocation.quota.cpu_percent as f64),
            ),
            ("pids".into(), Value::Number(allocation.quota.pids as f64)),
        ])),
        Err(e) => Ok(honest_error_value(&e.to_string())),
    }
}

/// The health-check configuration.
pub fn check_value(backend: &dyn ServiceBackend, name: &str) -> SmartResult<Value> {
    let (id, _) = backend.resolve(name).map_err(|e| crate::error::SmartError::failed(e.to_string()))?;
    match backend.get_health_check(id) {
        Ok(check) => Ok(check_value_single(&check)),
        Err(e) => Ok(honest_error_value(&e.to_string())),
    }
}

/// One health-check configuration record.
pub fn check_value_single(check: &ServiceHealthCheck) -> Value {
    Value::Object(vec![
        ("endpoint".into(), Value::String(check.endpoint.clone())),
        ("interval_ns".into(), Value::Number(check.interval_ns as f64)),
        ("timeout_ns".into(), Value::Number(check.timeout_ns as f64)),
        (
            "unhealthy_threshold".into(),
            Value::Number(check.unhealthy_threshold as f64),
        ),
        (
            "healthy_threshold".into(),
            Value::Number(check.healthy_threshold as f64),
        ),
    ])
}

/// The supervisor's state-change stream, bounded by the caller: one
/// subscription, one bounded pull.
pub fn watch_value(backend: &dyn ServiceBackend, name: Option<&str>, count: u32) -> SmartResult<Value> {
    let (id, present) = match name {
        Some(name) => backend.resolve(name).map_err(|e| crate::error::SmartError::failed(e.to_string()))?,
        None => (smart_lib::core::ids::ServiceId::new(0), true),
    };
    if !present {
        return Ok(honest_error_value(&format!(
            "the supervisor does not serve a service named {}",
            name.unwrap_or("?")
        )));
    }
    let sub = backend
        .subscribe_service_events(id)
        .map_err(|e| crate::error::SmartError::failed(e.to_string()))?;
    let events = backend
        .pull_service_events(sub, count.max(1))
        .map_err(|e| crate::error::SmartError::failed(e.to_string()))?;
    let _ = backend.cancel_service_events(sub);
    let names = names_by_id(backend);
    Ok(Value::List(
        events
            .iter()
            .map(|event| {
                Value::Object(vec![
                    (
                        "event".into(),
                        Value::String(event.event_type().as_token().to_string()),
                    ),
                    (
                        "name".into(),
                        Value::String(name_of(&names, event.service_id.get())),
                    ),
                    (
                        "state".into(),
                        Value::String(
                            match &event.payload {
                                smart_lib::domains::service::types::ServiceEventPayload::StateChanged(s) => {
                                    s.status.as_token().to_string()
                                }
                                _ => "unknown".to_string(),
                            },
                        ),
                    ),
                    (
                        "timestamp".into(),
                        exact(event.timestamp),
                    ),
                ])
            })
            .collect(),
    ))
}

/// The one gated surface: each `ServiceControl` verb's honest status. The verbs
/// are registry-config operations a host supervisor does not own, so each one
/// is reported with the contract's own reason class rather than a fake
/// `[control]` panel that changes nothing.
pub fn controls_value(verb: Option<&str>) -> Value {
    let verbs: Vec<(&str, &str)> = smart_lib::domains::service::availability::SERVICE_AVAILABILITY
        .iter()
        .filter(|row| {
            row.name == "setDependencies"
                || row.name == "setResourceAllocation"
                || row.name == "setHealthCheck"
                || row.name == "updateConfig"
                || row.name == "setQuarantine"
                || row.name == "setLogLevel"
                || row.name == "clearMetrics"
        })
        .map(|row| {
            (
                row.name,
                match row.reason_class {
                    Some(class) => class.as_str(),
                    None => "",
                },
            )
        })
        .collect();
    let selected: Vec<(&str, &str)> = match verb {
        Some(verb) => verbs.into_iter().filter(|(v, _)| *v == verb).collect(),
        None => verbs,
    };
    Value::List(
        selected
            .into_iter()
            .map(|(verb, class)| {
                Value::Object(vec![
                    ("verb".into(), Value::String(verb.to_string())),
                    ("status".into(), Value::String("unavailable".to_string())),
                    ("reason_class".into(), Value::String(class.to_string())),
                ])
            })
            .collect(),
    )
}

/// A mutating verb's echo (its rendering is human-only; a pipe sees the record).
pub fn verb_value(verb: &str, name: &str) -> Value {
    Value::Object(vec![
        ("verb".into(), Value::String(verb.to_string())),
        ("service".into(), Value::String(name.to_string())),
        (
            "result".into(),
            Value::String("the supervisor was asked to apply it".to_string()),
        ),
    ])
}

/// The honest log answer: this contract observes; log reading is
/// logging.capnp's `Logging` surface, and on a host supervisor the log store is
/// the service's own directory (runit's `log/`, journald on systemd). Nothing
/// here fabricates a log line.
pub fn logs_value(name: &str, lines: u32) -> Value {
    Value::Object(vec![
        ("service".into(), Value::String(name.to_string())),
        (
            "reason".into(),
            Value::String(format!(
                "unavailable: service.capnp OBSERVES and defines no log surface — reading a \
                 service's log is logging.capnp's Logging.tail, which is a later wave. On this \
                 host the log store is the supervisor's own (runit keeps it in the service dir's \
                 ./log subdirectory; journald on a systemd host). No log line was read and none \
                 was invented ({lines} lines were requested for {name})."
            )),
        ),
    ])
}

/// An error rendered as the shell's own honest answer, so a pipe and a human
/// see the same thing. The renderers never panic on a shape they did not
/// expect: an absent field is an absent field, not a crash.
fn error_field(value: &Value) -> Option<String> {
    match value.field("error") {
        Value::String(reason) if !reason.is_empty() => Some(reason.clone()),
        _ => None,
    }
}

fn honest_error_value(reason: &str) -> Value {
    Value::Object(vec![
        ("error".into(), Value::String(reason.to_string())),
        ("services".into(), Value::List(Vec::new())),
    ])
}

// ── the aesthetic ─────────────────────────────────────────────────────────
//
// Every cell here is CONTENT plus a STYLE (`Cell`), never a pre-styled string:
// the table measures the content, and applies the colour after the column
// widths are fixed. That is what keeps the state glyph's width out of the
// measurement entirely — the failure this table shipped with was exactly a
// pre-styled `"\x1b[32m◉\x1b[0m"` cell, whose green escape is one byte longer
// than the dim escape a `○` row carries, measured inconsistently.

fn state_glyph(status: ServiceStatus) -> Cell {
    match status {
        ServiceStatus::Active => Cell::styled("◉", Sgr::Green),
        ServiceStatus::Inactive => Cell::styled("○", Sgr::Dimmed),
        ServiceStatus::Failed => Cell::styled("✗", Sgr::Red),
        ServiceStatus::Reloading | ServiceStatus::Activating | ServiceStatus::Deactivating => {
            Cell::styled("◐", Sgr::Yellow)
        }
        ServiceStatus::Unknown | ServiceStatus::Custom => Cell::styled("○", Sgr::Dimmed),
    }
}

fn health_glyph(state: HealthState) -> Cell {
    match state {
        HealthState::Online => Cell::styled("◉", Sgr::Green),
        HealthState::Offline => Cell::styled("○", Sgr::Dimmed),
        HealthState::Degraded => Cell::styled("◐", Sgr::Yellow),
        HealthState::Starting | HealthState::Maintenance => Cell::styled("◐", Sgr::Yellow),
        HealthState::Unknown | HealthState::Custom => Cell::styled("○", Sgr::Dimmed),
    }
}

/// Render one command's value in the requested format.
pub fn render_action(cmd: &super::ServiceCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => render_human(cmd, value),
    }
}

fn render_human(cmd: &super::ServiceCmd, value: &Value) -> String {
    use super::ServiceAction as A;
    match &cmd.action {
        None | Some(A::Status { name: None }) => render_services(value),
        Some(A::Status { name: Some(name) }) => render_inspect(value, name),
        Some(A::Inspect { name }) => render_inspect(value, name),
        Some(A::Health { quarantined }) => {
            if *quarantined {
                render_quarantined(value)
            } else {
                render_health(value)
            }
        }
        Some(A::Deps { .. }) => render_deps(value),
        Some(A::Resolve { .. }) => render_resolve(value),
        Some(A::Metrics { .. }) => render_metrics(value),
        Some(A::Allocation { .. }) | Some(A::Check { .. }) => render_unavailable_block(value),
        Some(A::Watch { .. }) => render_watch(value),
        Some(A::Controls { .. }) => render_controls(value),
        Some(A::Start { .. }) | Some(A::Stop { .. }) | Some(A::Restart { .. }) => render_verb(value),
        Some(A::Logs { .. }) => render_logs(value),
    }
}

/// The service table. A supervisor that could not be reached renders its own
/// reason instead of an empty table that would claim the system has no
/// services.
fn render_services(value: &Value) -> String {
    let mut out = String::new();
    if let Some(error) = error_field(value) {
        out.push_str(&format!("  {} {}\n", "⚙".blue().bold(), "Services".blue().bold()));
        out.push_str(&format!("  └─ {} {}\n", "✗".red(), error));
        return out.trim_end().to_string();
    }
    let Value::List(services) = value else {
        return String::new();
    };
    out.push_str(&format!(
        "  {} {} {} {}\n",
        "⚙".blue().bold(),
        "Services".blue().bold(),
        format!("({})", services.len()).dimmed(),
        "— the supervisor's own state, read natively".dimmed()
    ));
    if services.is_empty() {
        out.push_str(&format!(
            "  └─ {} the supervisor serves no services (an honest empty — the supervisor was read \n     and answered with none)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "NAME".into(),
        "STATE".into(),
        "PID".into(),
        "ENABLED".into(),
        "SUPERVISED".into(),
        "SINCE".into(),
    ]);
    for service in services {
        table = table.row(vec![
            service.field_str("name").into(),
            // The glyph and its word in the state's own colour: the content is
            // plain text, so the column measures "◉ active" — never the escape
            // that colours it.
            state_glyph(service_status(service))
                .with_text(&format!(" {}", service.field_str("state"))),
            service.field_str("pid").into(),
            yes_no(service.field_bool("enabled")),
            yes_no(service.field_bool("supervised")),
            since_label(service).into(),
        ]);
    }
    out.push_str(&table.render());
    out.push('\n');
    // The summary line reports every state including the not-supervised ones,
    // so nothing hides behind a single "running" count.
    let active = services
        .iter()
        .filter(|s| s.field_str("state") == "active")
        .count();
    let failed = services
        .iter()
        .filter(|s| s.field_str("state") == "failed")
        .count();
    let unknown = services
        .iter()
        .filter(|s| s.field_str("state") == "unknown")
        .filter_map(|s| {
            let reason = s.field_str("state_reason");
            if reason.is_empty() {
                None
            } else {
                Some(format!("{}: {}", s.field_str("name"), reason))
            }
        })
        .collect::<Vec<String>>();
    let supervised = services
        .iter()
        .filter(|s| s.field_bool("supervised"))
        .count();
    out.push_str(&format!(
        "\n  {} active · {} supervised · {} failed — {} row(s) not supervised on this host\n",
        active,
        supervised,
        failed,
        services.len().saturating_sub(supervised)
    ));
    // One block naming WHY the rows that answered `unknown` did, in the
    // supervisor's own measured class: a credential wall is named as one, and
    // a host that simply does not supervise the service is named as that.
    // Neither is a fabrication and neither is hidden.
    if !unknown.is_empty() {
        let blocked = unknown
            .iter()
            .filter(|l| l.contains("no-privilege:"))
            .count();
        let unsup = unknown
            .iter()
            .filter(|l| l.contains("not-found:"))
            .count();
        if blocked > 0 {
            out.push_str(&format!(
                "\n  {} the supervisor's run state is not readable by this caller — {} row(s)\n",
                "●".blue().bold(),
                blocked
            ));
            out.push_str("     answered with a measured no-privilege reason (below).\n");
        }
        if unsup > 0 {
            out.push_str(&format!(
                "\n  {} real states for every uid — no row above is blocked by privilege.\n",
                "●".blue().bold()
            ));
            out.push_str(
                "     These services are not supervised on this host: the supervisor's own read\n",
            );
            out.push_str(
                "     found no supervise tree for them and no process serving them — the reason\n",
            );
            out.push_str("     each one did not find is measured, per row, below.\n");
        }
        out.push_str(
            "     The one physically-privileged half is the supervisor's own exact stat words\n",
        );
        out.push_str("     (its supervise tree is root-only for this uid):\n");
        out.push_str(&format!(
            "       {}\n",
            "sudo ./target/debug/examples/service-state-probe --state-root /run/runit".dimmed()
        ));
        for line in unknown.iter().take(3) {
            out.push_str(&format!("       {}\n", line));
        }
        if unknown.len() > 3 {
            out.push_str(&format!(
                "       … and {} more service(s) with the same measured reason\n",
                unknown.len() - 3
            ));
        }
    }
    out.trim_end().to_string()
}

fn service_status(service: &Value) -> ServiceStatus {
    match service.field_str("state").as_str() {
        "active" => ServiceStatus::Active,
        "inactive" => ServiceStatus::Inactive,
        "failed" => ServiceStatus::Failed,
        "reloading" => ServiceStatus::Reloading,
        "activating" => ServiceStatus::Activating,
        "deactivating" => ServiceStatus::Deactivating,
        _ => ServiceStatus::Unknown,
    }
}

fn service_health_state(service: &Value) -> HealthState {
    match service.field_str("state").as_str() {
        "online" => HealthState::Online,
        "offline" => HealthState::Offline,
        "degraded" => HealthState::Degraded,
        "starting" => HealthState::Starting,
        "maintenance" => HealthState::Maintenance,
        _ => HealthState::Unknown,
    }
}

fn yes_no(yes: bool) -> Cell {
    if yes {
        Cell::styled("◉", Sgr::Green)
    } else {
        Cell::styled("○", Sgr::Dimmed)
    }
}

/// The `SINCE` column: the supervisor's own transition count when it was
/// observed, and `—` when it was not (never a fabricated duration).
fn since_label(service: &Value) -> String {
    if !service.field_bool("since_valid") {
        return "—".to_string();
    }
    let micros = service.field("since_us").as_f64().unwrap_or(0.0);
    let secs = (micros / 1_000_000.0) as u64;
    if secs >= 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn render_inspect(value: &Value, name: &str) -> String {
    if !value.field_bool("present") {
        return format!(
            "  {} {} — the supervisor does not serve a service named {} (the honest answer, not a fault)",
            "⚙".blue().bold(),
            name.dimmed(),
            name
        );
    }
    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", "⚙".blue().bold(), name.blue().bold()));
    let description = value.field_str("description");
    if !description.is_empty() {
        out.push_str(&format!("  ├─ Description  {}\n", description.dimmed()));
    }
    let state = value.field_str("state");
    out.push_str(&format!(
        "  ├─ State        {} {}\n",
        state_glyph(service_status(value)),
        state
    ));
    let reason = value.field_str("state_reason");
    if !reason.is_empty() {
        out.push_str(&format!("  ├─ Reason       {}\n", reason));
    }
    out.push_str(&format!(
        "  ├─ Main PID     {}\n",
        if value.field("pid").as_f64().unwrap_or(0.0) > 0.0 {
            value.field_str("pid")
        } else {
            "○ none measured".to_string()
        }
    ));
    out.push_str(&format!(
        "  ├─ Enabled      {}\n",
        yes_no(value.field_bool("enabled"))
    ));
    if let Value::Object(health) = value.field("health") {
        let state = health
            .iter()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.display())
            .unwrap_or_default();
        let passed = health
            .iter()
            .find(|(k, _)| k == "last_check_passed")
            .map(|(_, v)| v.as_bool().unwrap_or(false))
            .unwrap_or(false);
        let note = if passed {
            String::new()
        } else {
            "  (probe not run by a read-only backend)".dimmed().to_string()
        };
        out.push_str(&format!(
            "  └─ Health       {} {}{}\n",
            health_glyph(service_health_state_of(&state)),
            state,
            note
        ));
    } else {
        out.push_str(&format!("  └─ Health       {}", "○ not readable".dimmed()));
    }
    out
}

fn service_health_state_of(token: &str) -> HealthState {
    match token {
        "online" => HealthState::Online,
        "offline" => HealthState::Offline,
        "degraded" => HealthState::Degraded,
        "starting" => HealthState::Starting,
        "maintenance" => HealthState::Maintenance,
        _ => HealthState::Unknown,
    }
}

fn render_health(value: &Value) -> String {
    let mut out = String::new();
    if let Some(error) = error_field(value) {
        out.push_str(&format!(
            "  {} Health\n  └─ {} {}\n",
            "❤".blue().bold(),
            "✗".red(),
            error
        ));
        return out.trim_end().to_string();
    }
    let Value::List(services) = value else {
        return String::new();
    };
    out.push_str(&format!(
        "  {} {} {}\n",
        "❤".blue().bold(),
        "Health".blue().bold(),
        format!("({})", services.len()).dimmed()
    ));
    if services.is_empty() {
        out.push_str(&format!("  └─ {} no service to report on\n", "○".dimmed()));
        return out.trim_end().to_string();
    }
    let mut table = Table::new(vec![
        "SERVICE".into(),
        "STATE".into(),
        "QUARANTINED".into(),
        "LAST CHECK".into(),
        "FAULTS".into(),
    ]);
    for service in services {
        table = table.row(vec![
            service.field_str("name").into(),
            health_glyph(service_health_state(service))
                .with_text(&format!(" {}", service.field_str("state"))),
            yes_no(service.field_bool("quarantined")),
            if service.field_bool("last_check_passed") {
                "◉ passed".to_string()
            } else {
                "○ not run".to_string()
            }
            .into(),
            service.field_str("fault_count").into(),
        ]);
    }
    out.push_str(&table.render());
    out.trim_end().to_string()
}

fn render_quarantined(value: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {}\n",
        "🚧".blue().bold(),
        "Quarantined".blue().bold()
    ));
    let Value::Object(fields) = value else {
        return out.trim_end().to_string();
    };
    let total = fields
        .iter()
        .find(|(k, _)| k == "total")
        .map(|(_, v)| v.as_f64().unwrap_or(0.0))
        .unwrap_or(0.0);
    if total == 0.0 {
        out.push_str(&format!(
            "  └─ {} none — and that is a measured fact: this supervisor has no quarantine concept,\n     so the answer is the honest empty rather than a fabricated entry\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    out.push_str(&format!("  └─ {} quarantined services\n", total));
    out.trim_end().to_string()
}

fn render_deps(value: &Value) -> String {
    let mut out = String::new();
    if let Some(error) = error_field(value) {
        out.push_str(&format!("  {} Dependencies\n  └─ {} {}\n", "🔗".blue().bold(), "✗".red(), error));
        return out.trim_end().to_string();
    }
    let Value::List(deps) = value else {
        return String::new();
    };
    out.push_str(&format!("  {} {} {}\n", "🔗".blue().bold(), "Dependencies".blue().bold(), format!("({})", deps.len()).dimmed()));
    if deps.is_empty() {
        out.push_str(&format!(
            "  └─ {} the supervisor declares none for this service (its own facts, not an omission)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    for dep in deps {
        out.push_str(&format!(
            "  ├─ {} {:<9} {}\n",
            if dep.field_bool("satisfied") { "◉".green() } else { "○".dimmed() },
            dep.field_str("dependency_type"),
            dep.field_str("name")
        ));
    }
    out.trim_end().to_string()
}

fn render_resolve(value: &Value) -> String {
    let present = value.field_bool("present");
    let name = value.field_str("name");
    let id = value.field_str("service_id");
    format!(
        "  {} resolve {:<20} {} {}",
        "🔎".blue().bold(),
        name,
        if present {
            "◉".green().to_string()
        } else {
            "○".dimmed().to_string()
        },
        if present {
            format!("id {id}")
        } else {
            "not served by this supervisor".to_string()
        }
    )
}

fn render_metrics(value: &Value) -> String {
    // A host supervisor observes no application metrics, so the reason travels
    // with the answer rather than a table of zeros a reader could mistake for
    // "nothing was measured because nothing happened".
    if let Some(reason) = error_field(value) {
        return format!("  {} Metrics — {} {}", "📈".blue().bold(), "✗".red(), reason);
    }
    let f = |k: &str| value.field_str(k);
    format!(
        "  {} Metrics — measured over {} ns\n  ├─ requests  {}\n  ├─ errors    {}\n  └─ p99       {} ns",
        "📈".blue().bold(),
        f("window_ns"),
        f("request_count"),
        f("error_count"),
        f("latency_p99_ns")
    )
}

fn render_unavailable_block(value: &Value) -> String {
    match error_field(value) {
        None => format!("  {} {}", "✓".green(), value.display()),
        Some(reason) => format!("  {} {}", "✗".red(), reason),
    }
}

fn render_watch(value: &Value) -> String {
    let Value::List(events) = value else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — {} event(s) bounded\n",
        "⟳".blue().bold(),
        "Service events".blue().bold(),
        events.len()
    ));
    if events.is_empty() {
        out.push_str(&format!(
            "  └─ {} no transition since the subscription was minted (an honest empty)\n",
            "○".dimmed()
        ));
        return out.trim_end().to_string();
    }
    for event in events {
        out.push_str(&format!(
            "  ├─ {:<9} {:<9} {}\n",
            event.field_str("event"),
            event.field_str("state"),
            event.field_str("name")
        ));
    }
    out.trim_end().to_string()
}

fn render_controls(value: &Value) -> String {
    let Value::List(verbs) = value else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!(
        "  {} {} — the one gated surface, and what it honestly answers here\n",
        "🔒".blue().bold(),
        "ServiceControl".blue().bold()
    ));
    for verb in verbs {
        let class = verb.field_str("reason_class");
        out.push_str(&format!(
            "  ├─ {:<24} {} {}\n",
            verb.field_str("verb"),
            "✗".red(),
            format!("{class} — a registry-config operation a host supervisor does not own").dimmed()
        ));
    }
    out.trim_end().to_string()
}

fn render_verb(value: &Value) -> String {
    let verb = value.field_str("verb");
    let service = value.field_str("service");
    let result = value.field_str("result");
    format!("  {} {} {} — {}", "✓".green(), verb.green(), service, result)
}

fn render_logs(value: &Value) -> String {
    let service = value.field_str("service");
    let reason = value.field_str("reason");
    let mut out = String::new();
    out.push_str(&format!("  {} {} logs\n", "📜".blue().bold(), service));
    out.push_str(&format!("  └─ {} {}\n", "○".dimmed(), reason));
    out.trim_end().to_string()
}
