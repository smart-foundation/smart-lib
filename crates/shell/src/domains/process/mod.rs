//! `process` — the domain command module on the NEW engine.
//!
//! It renders from the engine's OWNED types (`ProcessInfo`, `ProcessStatus`,
//! `ProcessStats`, `ResourceUsage`, `ThreadInfo`, `MemoryMapEntry`,
//! `ProcessGroup`, `Quota`, `ExitStatus`) and reads the host's own `/proc`
//! natively: `/proc/<pid>/{stat,status,cmdline,statm,environ,maps,task/<tid>/stat}`,
//! `btime` from `/proc/stat`, the host's own page size and `CLK_TCK` through
//! `sysconf`, and `waitpid(WNOHANG)` for the contract's own "do not block".
//!
//! ## Every number is measured
//!
//! The archive body's hardcoded MEM/CPU pair is the named past this module
//! exists to end. The table's memory column is the kernel's own resident pages
//! (`statm` rss × the host's own page size); its time column is the kernel's own
//! tick counters (`utime + stime`) scaled by the host's own clock-tick rate;
//! the aggregate is measured across a bounded 20 ms window of the kernel's own
//! counters; the uptime is `btime` plus the process's own start time. A field
//! the kernel does not export per-thread stays 0 with the reason carried in the
//! engine's own words.
//!
//! ## The gated half, and the host's measured verdicts
//!
//! Signal delivery and a ptrace attach are host-mutating: both are gated, and
//! the engine answers a gate-free probe with the host's own measured verdict —
//! `kill(pid, 0)` for signal authority (signal 0 is the kernel's own null
//! signal), and `ptrace_scope` plus this process's capability set for attach.
//! Read memory is EACCES for a foreign uid on this host (measured), and that
//! is what it answers.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::core::ids::ProcessId;
use smart_lib::domains::process::types::{
    ExitStatus, ExecutionState, Pagination, ProcessFilter, ProcessInfo, SignalKind,
};
use smart_lib::domains::process::ProcessBackend;

/// The engine seam this shell speaks to — the per-domain backend trait object.
pub type Backend = dyn ProcessBackend;

/// Every page read this command module issues asks for everything (the shell
/// slices with the pipe engine); the backend still bounds each pull.
fn all() -> Pagination {
    Pagination {
        offset: 0,
        limit: u32::MAX,
    }
}

/// `smart process` — the processes the host runs, read from `/proc`, measured.
///
/// A bare pid (`smart process 1234`) is the inspect view, the way a process
/// table is actually used; the subcommands are the rest of the contract's read
/// surface.
#[derive(Parser, Debug)]
pub struct ProcessCmd {
    #[command(subcommand)]
    pub action: Option<ProcessAction>,
    /// One process to inspect (`smart process <pid>`).
    #[arg(value_name = "PID")]
    pub pid: Option<u64>,
    /// Only the processes whose name or command contains this text.
    #[arg(long)]
    name: Option<String>,
    /// Only the processes in this state (active/blocked/suspended/terminated).
    #[arg(long)]
    state: Option<String>,
    /// Only this uid's processes.
    #[arg(long)]
    user: Option<u64>,
}

#[derive(Subcommand, Debug)]
pub enum ProcessAction {
    /// One process's detail: the contract's whole read surface for one pid.
    Inspect {
        /// The process id.
        pid: u64,
    },
    /// The tree one process belongs to (`getProcessTree`).
    Tree {
        /// The process id to root the tree at (default: this process).
        pid: Option<u64>,
        /// How deep the tree descends.
        #[arg(long, default_value = "4")]
        depth: u32,
    },
    /// The threads of one process (`getThreads`).
    Threads {
        /// The process id.
        pid: u64,
    },
    /// The environment of one process (`getEnvironment`), rendered as the
    /// host's own NUL-separated pairs.
    Env {
        /// The process id.
        pid: u64,
        /// Hide values, keeping the names (a secret does not belong on a screen
        /// that may be shared).
        #[arg(long)]
        names_only: bool,
    },
    /// The memory map of one process (`getMemoryMap`) — readable for this uid's
    /// own processes only.
    Maps {
        /// The process id.
        pid: u64,
    },
    /// The resource limits of one process (`getResourceLimits`).
    Limits {
        /// The process id.
        pid: u64,
    },
    /// The resource accounting of one process (`getAccounting`).
    Accounting {
        /// The process id.
        pid: u64,
    },
    /// The kernel's own job-control groups (`listProcessGroups`).
    Groups,
    /// One group's members (`getProcessGroup`).
    Group {
        /// The process-group id.
        id: u64,
    },
    /// The aggregate statistics over every process (`getStats`), with the
    /// measured CPU over the kernel's own window.
    Stats {
        /// One uid's processes only (default: everyone).
        #[arg(long)]
        user: Option<u64>,
    },
    /// This host's measured ptrace verdict, in the host's own words.
    Ptrace,
    /// The measured signal-delivery verdict for one pid (the host's own
    /// `kill(pid, 0)` probe — a read, never a signal).
    CanSignal {
        /// The process id.
        pid: u64,
    },
    /// Deliver a signal to a process (host-mutating — gated; the measured
    /// verdict above decides whether the host would even accept it).
    Signal {
        /// The process id.
        pid: u64,
        /// The signal token: terminate, interrupt, kill, hangUp, quit,
        /// userDefined1, userDefined2, child, stop, continue.
        #[arg(long, default_value = "terminate")]
        signal: String,
    },
    /// A ptrace attach (host-mutating — gated; this host's own ptrace_scope
    /// decides, and the verdict is measured, not assumed).
    Attach {
        /// The process id.
        pid: u64,
    },
    /// A bounded wait for one process to exit (`wait`) — `waitpid(WNOHANG)`
    /// under the contract's own "do not block", or a bounded poll.
    Wait {
        /// The process id.
        pid: u64,
        /// How long to wait, in milliseconds (0 is the contract's own
        /// "do not block").
        #[arg(long, default_value = "0")]
        timeout_ms: u64,
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

/// Parse a signal token through the engine's own vocabulary (an unknown token
/// is the contract's `custom` arm, never a guessed signal).
fn signal_kind(token: &str) -> SignalKind {
    SignalKind::from_token(token)
}

/// The state token a caller names, parsed through the engine's own vocabulary.
fn state_filter(state: Option<&str>) -> SmartResult<ExecutionState> {
    match state {
        None => Ok(ExecutionState::Unknown),
        Some(token) => Ok(ExecutionState::from_token(token)),
    }
}

/// Evaluate the command's data half: the value the command produces, with no
/// rendering. This is what pipes consume.
pub fn evaluate(cmd: &ProcessCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    // `smart process <pid>` — the inspect view a bare pid names, before the
    // subcommand arms (the two never both appear).
    if let Some(pid) = cmd.pid {
        if !matches!(cmd.action, None | Some(ProcessAction::Inspect { .. })) {
            return Err(SmartError::invalid_argument(
                "a bare pid is the inspect view: `smart process <pid>` takes no other subcommand",
            ));
        }
        return render::inspect_value(backend, ProcessId::new(pid));
    }
    match &cmd.action {
        None => render::table_value(cmd, backend),
        Some(ProcessAction::Inspect { pid }) => render::inspect_value(backend, ProcessId::new(*pid)),
        Some(ProcessAction::Tree { pid, depth }) => render::tree_value(
            backend,
            ProcessId::new(pid.unwrap_or_else(|| std::process::id() as u64)),
            *depth,
        ),
        Some(ProcessAction::Threads { pid }) => render::threads_value(backend, ProcessId::new(*pid)),
        Some(ProcessAction::Env { pid, names_only }) => {
            render::env_value(backend, ProcessId::new(*pid), *names_only)
        }
        Some(ProcessAction::Maps { pid }) => render::maps_value(backend, ProcessId::new(*pid)),
        Some(ProcessAction::Limits { pid }) => render::limits_value(backend, ProcessId::new(*pid)),
        Some(ProcessAction::Accounting { pid }) => {
            render::accounting_value(backend, ProcessId::new(*pid))
        }
        Some(ProcessAction::Groups) => render::groups_value(backend),
        Some(ProcessAction::Group { id }) => render::group_value(backend, *id),
        Some(ProcessAction::Stats { user }) => {
            let uid = smart_lib::core::ids::UserId::new(user.unwrap_or(0));
            render::stats_value(backend, uid)
        }
        Some(ProcessAction::Ptrace) => render::ptrace_value(backend),
        Some(ProcessAction::CanSignal { pid }) => {
            render::can_signal_value(backend, ProcessId::new(*pid))
        }
        Some(ProcessAction::Signal { pid, signal }) => {
            guard_host_write(guard, &format!("process signal {pid}"))?;
            let kind = signal_kind(signal);
            let pid = ProcessId::new(*pid);
            // The engine's own gated delivery: it measures the host's verdict
            // first and refuses with the host's words when this uid may not.
            backend
                .signal_process(pid, kind)
                .map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("signal".into())),
                ("pid".into(), Value::Number(pid.get() as f64)),
                ("signal".into(), Value::String(kind.as_token().to_string())),
                ("delivered".into(), Value::Bool(true)),
            ]))
        }
        Some(ProcessAction::Attach { pid }) => {
            guard_host_write(guard, &format!("process attach {pid}"))?;
            backend
                .attach(ProcessId::new(*pid))
                .map_err(SmartError::from)?;
            Ok(render::verb_value("attach", &pid.to_string()))
        }
        Some(ProcessAction::Wait { pid, timeout_ms }) => {
            render::wait_value(backend, ProcessId::new(*pid), *timeout_ms)
        }
        Some(ProcessAction::Caps) => Ok(render::caps_value(backend)),
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(cmd: &ProcessCmd, format: &OutputFormat, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    let value = evaluate(cmd, backend, guard)?;
    if *format != OutputFormat::Raw {
        println!("{}", render::render_action(cmd, &value, *format));
    }
    Ok(value)
}

/// The host write this command performs, when it performs one: the verb and
/// the target the confirmation prompt shows.
pub fn write_intent(cmd: &ProcessCmd) -> Option<(String, String)> {
    use ProcessAction as P;
    Some(match &cmd.action {
        Some(P::Signal { pid, signal }) => {
            ("deliver a signal to".into(), format!("pid {pid} ({signal})"))
        }
        Some(P::Attach { pid }) => ("attach to".into(), format!("pid {pid}")),
        _ => return None,
    })
}

pub mod render;

/// The host-write gate (law #8, the C1 shape): a mutating verb proceeds only
/// when the caller accepted the write.
fn guard_host_write(guard: WriteGuard, verb: &str) -> SmartResult<()> {
    if guard.allowed {
        return Ok(());
    }
    Err(SmartError::not_supported(format!(
        "refusing to run `{verb}`: it CHANGES host state. The interactive `(y/N)` prompt asks with the target's \
         context beside it; a piped `y` line answers it, and `--force` accepts the \
         write explicitly (the live bring-back proof is founder-run)."
    )))
}

/// The process selector tree, rendered from the engine's owned types — the same
/// values the command surface renders, so a selector and a command can never
/// disagree. `@process/processes`, `@process/stats`, `@process/caps` resolve
/// from here.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let filter = ProcessFilter::default();
    let processes = backend.list_processes(&filter, all()).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        (
            "processes".into(),
            Value::List(processes.items.iter().map(render::process_value).collect()),
        ),
        (
            "stats".into(),
            render::stats_value(backend, smart_lib::core::ids::UserId::new(0))
                .unwrap_or_else(|_| Value::Null),
        ),
        ("caps".into(), render::caps_value(backend)),
    ]))
}

/// The listing filter a `--name`/`--state`/`--user` table builds, through the
/// engine's own filter vocabulary.
pub(crate) fn table_filter(
    name: Option<&str>,
    state: Option<&str>,
    user: Option<u64>,
) -> SmartResult<ProcessFilter> {
    Ok(ProcessFilter {
        name: name.unwrap_or_default().to_string(),
        state: state_filter(state)?,
        user: smart_lib::core::ids::UserId::new(user.unwrap_or(0)),
        min_cpu_usage: 0.0,
        min_memory_usage: 0,
    })
}

/// The `wait` timeout the contract's own `Duration` names: 0 is "do not block".
pub(crate) fn wait_timeout(ms: u64) -> smart_lib::core::values::Duration {
    smart_lib::core::values::Duration::from_nanos(ms.saturating_mul(1_000_000))
}

/// The exit status's own words (the contract's total union, rendered honestly).
pub(crate) fn exit_status_words(status: &ExitStatus) -> String {
    match status {
        ExitStatus::Exited(code) => format!("exited {code}"),
        ExitStatus::Signaled(signal) => format!("signalled {}", signal.as_token()),
        ExitStatus::TimedOut => "timed out".to_string(),
        ExitStatus::Custom(_) => "the contract's custom arm".to_string(),
    }
}

/// Re-export for the shell and main: the clap shape.
pub use self::{ProcessAction as Action, ProcessCmd as Cmd};

/// One engine `ProcessInfo` as a record: the values the table and the selector
/// tree both render, measured.
pub(crate) fn info_summary(info: &ProcessInfo) -> Value {
    Value::Object(vec![
        ("pid".into(), Value::Number(info.pid.get() as f64)),
        ("ppid".into(), Value::Number(info.ppid.get() as f64)),
        ("name".into(), Value::String(info.name.clone())),
        ("command".into(), Value::String(info.command.clone())),
        ("state".into(), Value::String(info.state.as_token().to_string())),
        ("uid".into(), Value::Number(info.user.get() as f64)),
        ("threads".into(), Value::Number(f64::from(info.threads))),
        ("mem".into(), Value::Number(info.memory_usage as f64)),
    ])
}
