//! `service` — the domain command module, rewritten onto the NEW engine.
//!
//! What changed from the original shell's module: it renders from the engine's
//! OWNED types (`ServiceInstanceInfo`, `ServiceInstanceState`, `ServiceHealth`,
//! `ServiceEvent`, …) instead of capnp readers, and its read path is the
//! engine's **native supervision read** — on this host runit's own supervise
//! tree, on a systemd host the system bus, on openrc its own state files. The
//! original shell's `Command::new("sv" | "systemctl")` subprocess shellout is
//! gone: there is no subprocess anywhere in this domain.
//!
//! Host-mutating verbs (`start`, `stop`, `restart`) are gated exactly as
//! `network`'s and `clipboard`'s are: the operator must accept the write
//! (`--force` one-shot, or confirm in the REPL); the live bring-back proof of a
//! write is the founder's (charter law #8), and on this host the gate is the
//! honest answer — the supervisor's control tree is root-only for uid 1000.
//!
//! What this host cannot serve is answered in the shell's own aesthetic:
//! a root-only supervise tree answers `○ unknown` with the measured reason
//! naming the denied path, the registry-config surface (quota, health-check
//! configuration, metrics) answers `✗ unavailable` with the contract's own
//! reason class, and sysvinit-only concepts answer with the supervisor's name
//! and its absence. Never a fake empty, never a fabricated state.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::service::ServiceBackend;
use smart_lib::domains::service::types::Pagination;

/// The engine seam this shell speaks to — the corrected architecture's rule: the
/// shell embeds its engine in-process through the per-domain backend trait
/// object. Without the feature the seam is the unit type, so every signature
/// still compiles (and every path answers honestly that the engine half is not
/// built).
pub type Backend = dyn ServiceBackend;

/// Every page read this command module issues asks for everything (the shell
/// slices with the pipe engine); the backend still bounds each pull.
fn all() -> Pagination {
    Pagination {
        offset: 0,
        limit: u32::MAX,
    }
}

/// `smart service` — the runtime observation of the services a host supervisor
/// runs, and the supervisor's own control surface behind the host-write gate.
#[derive(Parser, Debug)]
pub struct ServiceCmd {
    #[command(subcommand)]
    pub action: Option<ServiceAction>,
}

#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// The services the supervisor runs, or one service's state when a name is
    /// given (`service status` is the table; `service status <name>` inspects
    /// one service).
    Status {
        /// The service's name, to inspect one service instead of listing them.
        name: Option<String>,
    },
    /// One service's state and health.
    Inspect {
        /// The service's name as the supervisor spells it.
        name: String,
    },
    /// The bounded health listing (`listHealth`), or only the quarantined ones.
    Health {
        /// Only the services that are running but not permitted to serve.
        #[arg(long)]
        quarantined: bool,
    },
    /// The dependencies one service declares (`getDependencies`).
    Deps {
        /// The service's name.
        name: String,
    },
    /// Resolve a name to an identity (`resolve`).
    Resolve {
        /// The service's name.
        name: String,
    },
    /// The runtime measurements over a declared window (`getMetrics`).
    Metrics {
        /// The service's name.
        name: String,
        /// The window in seconds (the contract declares it, so two backends
        /// cannot answer "since when?" differently).
        #[arg(long, default_value = "60")]
        window_secs: u64,
    },
    /// The declared resource allocation (`getResourceAllocation`).
    Allocation {
        /// The service's name.
        name: String,
    },
    /// The health-check configuration (`getHealthCheck`).
    Check {
        /// The service's name.
        name: String,
    },
    /// The supervisor's state-change stream, bounded by the caller.
    Watch {
        /// The service's name (omit for every service).
        name: Option<String>,
        /// How many events to take before stopping.
        #[arg(long, default_value = "5")]
        count: u32,
    },
    /// The one gated surface: each `ServiceControl` verb's honest status.
    Controls {
        /// One verb to inspect (`setDependencies`, `setQuarantine`, …).
        verb: Option<String>,
    },
    /// Start a service (host-mutating — gated; founder-run live).
    Start {
        /// The service's name.
        name: String,
    },
    /// Stop a service (host-mutating — gated; founder-run live).
    Stop {
        /// The service's name.
        name: String,
    },
    /// Restart a service (host-mutating — gated; founder-run live).
    Restart {
        /// The service's name.
        name: String,
    },
    /// Read a service's log. This contract OBSERVES; log reading is
    /// logging.capnp's surface, so this verb answers honestly where it is.
    Logs {
        /// The service's name.
        name: String,
        /// How many lines to take.
        #[arg(long, default_value = "20")]
        lines: u32,
    },
}

/// Whether the caller accepted the host write (the REPL prompts; one-shot mode
/// passes the `--force` flag through) — the C1 gate, re-used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard {
    pub allowed: bool,
}

/// The supervisor's identity and capabilities, published so a caller can tell
/// a native read from a decided absence before any method is called.
pub fn supervisor_value(backend: &Backend) -> Value {
    let identity = backend.supervisor();
    let caps = backend.supervision_caps();
    Value::Object(vec![
        ("supervisor".into(), Value::String(identity.supervisor)),
        ("service_root".into(), Value::String(identity.service_root)),
        ("declared_root".into(), Value::String(identity.declared_root)),
        ("state_root".into(), Value::String(identity.state_root)),
        (
            "can_enumerate".into(),
            Value::Bool(caps.can_enumerate),
        ),
        ("can_read_state".into(), Value::Bool(caps.can_read_state)),
        ("can_control".into(), Value::Bool(caps.can_control)),
    ])
}

/// Evaluate the command's data half: the value the command produces, with no
/// rendering. This is what pipes consume.
pub fn evaluate(cmd: &ServiceCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None | Some(ServiceAction::Status { name: None }) => render::services_value(backend, all()),
        Some(ServiceAction::Status { name: Some(name) }) => render::inspect_value(backend, name),
        Some(ServiceAction::Inspect { name }) => render::inspect_value(backend, name),
        Some(ServiceAction::Health { quarantined }) => {
            if *quarantined {
                render::quarantined_value(backend, all())
            } else {
                render::health_value(backend, all())
            }
        }
        Some(ServiceAction::Deps { name }) => render::deps_value(backend, name),
        Some(ServiceAction::Resolve { name }) => Ok(render::resolve_value(backend, name)?),
        Some(ServiceAction::Metrics { name, window_secs }) => {
            render::metrics_value(backend, name, *window_secs)
        }
        Some(ServiceAction::Allocation { name }) => render::allocation_value(backend, name),
        Some(ServiceAction::Check { name }) => render::check_value(backend, name),
        Some(ServiceAction::Watch { name, count }) => render::watch_value(backend, name.as_deref(), *count),
        Some(ServiceAction::Controls { verb }) => Ok(render::controls_value(verb.as_deref())),
        Some(ServiceAction::Start { name }) => {
            guard_host_write(guard, &format!("service start {name}"))?;
            backend.apply_lifecycle(service_id(backend, name)?, name, LifecycleVerb::Start)?;
            Ok(render::verb_value("start", name))
        }
        Some(ServiceAction::Stop { name }) => {
            guard_host_write(guard, &format!("service stop {name}"))?;
            backend.apply_lifecycle(service_id(backend, name)?, name, LifecycleVerb::Stop)?;
            Ok(render::verb_value("stop", name))
        }
        Some(ServiceAction::Restart { name }) => {
            guard_host_write(guard, &format!("service restart {name}"))?;
            backend.apply_lifecycle(service_id(backend, name)?, name, LifecycleVerb::Restart)?;
            Ok(render::verb_value("restart", name))
        }
        Some(ServiceAction::Logs { name, lines }) => Ok(render::logs_value(name, *lines)),
    }
}

/// The lifecycle verbs the shell's host-write gate guards.
use smart_lib::domains::service::LifecycleVerb;

/// The engine's identity for a name, so a mutating verb addresses the service
/// the supervisor knows rather than a name the shell invented.
fn service_id(backend: &Backend, name: &str) -> SmartResult<smart_lib::core::ids::ServiceId> {
    match backend.resolve(name) {
        Ok((id, true)) => Ok(id),
        Ok((_, false)) => Ok(smart_lib::core::ids::ServiceId::new(0)),
        Err(e) => Err(SmartError::failed(e.to_string())),
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(cmd: &ServiceCmd, format: &OutputFormat, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
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

/// The service selector tree, rendered from the engine's owned types — the same
/// values the command surface renders, so a selector and a command can never
/// disagree. `@service/services`, `@service/supervisor`, `@service/quarantined`
/// resolve from here.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let services = render::services_value(backend, all())?;
    Ok(Value::Object(vec![
        ("services".into(), services),
        ("supervisor".into(), supervisor_value(backend)),
    ]))
}

/// Re-export for the shell and main: the clap shape.
pub use self::{ServiceAction as Action, ServiceCmd as Cmd};
