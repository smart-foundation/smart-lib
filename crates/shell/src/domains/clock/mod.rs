//! `clock` — the domain command module on the NEW engine.
//!
//! It renders from the engine's OWNED types (`ClockInfo`, `ClockSyncInfo`,
//! `ClockSecurityInfo`, `ClockDomainInfo`, `TimerSpec`, `TimerHandle`,
//! `TimerEvent`, `Alarm`, `VirtualClockHandle`) and speaks the kernel's own
//! clocks directly — `clock_gettime` for the realtime and monotonic readings,
//! `clock_getres` for the resolution, `timerfd_create` + `timerfd_settime` for
//! the timers (so a periodic timer that fired three times reports **3**, the
//! kernel's own expiration count), `nanosleep` on the caller's own thread,
//! and `/proc/stat`'s `btime` for the uptime. There is no `date` fork, no
//! `hwclock` fork and no `timedatectl` fork.
//!
//! ## What this host measures
//!
//! The sync state is measured, not assumed: a `chronyd` process IS running on
//! this host (pid 711), so `getClockSync` reports that fact, while its state
//! directory is root-only for this uid — so `synced` stays false and `stratum`
//! stays 0 with the measured reason beside them rather than being invented.
//! Drift is measured over a bounded window and says so. Timer persistence has
//! no host store here, which is a physical limit and is named as one per row.
//!
//! ## The removal verbs are gated
//!
//! Creating a timer is arming a `timerfd` this process owns and creating an
//! alarm is this process's own store — both live and shown. REMOVING one is
//! gated exactly as every other mutating verb is (charter law #8): the live
//! bring-back proof is founder-run.

use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::core::ids::AlarmId;
use smart_lib::core::values::{Duration, Timestamp};
use smart_lib::domains::clock::types::{
    Alarm, ClockDomainKind, HolidayBehavior, TimerAnchor, TimerSpec, VirtualClockConfig,
};
use smart_lib::domains::clock::ClockBackend;

/// The engine seam this shell speaks to: the contract's own [`ClockBackend`],
/// plus the sync-evidence surface the Linux backend publishes beside it — the
/// host's own measured words about its synchronization daemon, which the trait's
/// default answers as an empty string.
pub trait SyncEvidenceSurface {
    /// The measured words behind the sync verdict: which daemon, which pid,
    /// and what remains unreadable.
    fn sync_evidence(&self) -> String;
}

impl SyncEvidenceSurface for smart_lib::platforms::linux::clock::LinuxClockBackend {
    fn sync_evidence(&self) -> String {
        smart_lib::platforms::linux::clock::LinuxClockBackend::sync_evidence(self)
    }
}

/// The seam the clock command module speaks to.
pub trait ClockSeam: ClockBackend + SyncEvidenceSurface {}
impl ClockSeam for smart_lib::platforms::linux::clock::LinuxClockBackend {}

/// The engine seam this shell speaks to — the per-domain backend trait object.
pub type Backend = dyn ClockSeam;

/// Every page read this command module issues asks for everything (the shell
/// slices with the pipe engine); the backend still bounds each pull.
fn all() -> smart_lib::domains::clock::types::Pagination {
    smart_lib::domains::clock::types::Pagination {
        offset: 0,
        limit: u32::MAX,
    }
}

/// `smart clock` — the host's clocks, timers and alarms, read from the kernel.
#[derive(Parser, Debug)]
pub struct ClockCmd {
    #[command(subcommand)]
    pub action: Option<ClockAction>,
}

#[derive(Subcommand, Debug)]
pub enum ClockAction {
    /// The measured state of this host's clocks (the default view).
    Info,
    /// The wall-clock reading (`realtimeNow`) — the kernel's own
    /// `CLOCK_REALTIME`.
    Now,
    /// The monotonic reading (`monotonicNow`) — immune to NTP and DST.
    Monotonic,
    /// Time since the last boot (`btime` plus the monotonic base).
    Uptime,
    /// The measured resolution (`clock_getres`).
    Resolution,
    /// The measured NTP/PTP synchronization state (`getClockSync`), with the
    /// host's own words.
    Sync,
    /// The measured clock-security state (tamper / secure source / hardware RNG).
    Security,
    /// The clock domains this host can serve (`listClockDomains`).
    Domains,
    /// Quality metrics for one clock source (`getClockQuality`).
    Quality {
        /// The source token, e.g. `tsc`.
        source: String,
    },
    /// The measured drift between two clock domains (`getClockDrift`).
    Drift {
        /// The first domain.
        a: String,
        /// The second domain.
        b: String,
    },
    /// Sleep for a duration on the caller's own thread (`sleep`).
    Sleep {
        /// How long to sleep, in milliseconds.
        millis: u64,
    },
    /// Arm a real timerfd timer (`createTimer`) and report its first fire.
    Timer {
        /// The subcommand: create, check, list, cancel.
        #[command(subcommand)]
        action: TimerAction,
    },
    /// A cancellable sleep (`sleepCancellable`) and its cancel.
    SleepCancellable {
        /// How long to sleep, in milliseconds.
        millis: u64,
        /// Cancel it after this many milliseconds (the removal half is gated).
        #[arg(long)]
        cancel_after: Option<u64>,
    },
    /// Create a virtual (software) clock for testing (`createVirtualClock`).
    Virtual {
        #[command(subcommand)]
        action: VirtualAction,
    },
    /// The alarm surface (`alarms @19`): this process's own alarm store.
    Alarm {
        #[command(subcommand)]
        action: AlarmAction,
    },
    /// The timer event stream (`subscribeTimers` + bounded pulls).
    Watch {
        /// How many pull rounds to take before stopping.
        #[arg(long, default_value = "3")]
        count: u32,
        /// The pause between rounds, in milliseconds.
        #[arg(long, default_value = "200")]
        interval: u64,
    },
    /// This backend's honest capability tokens.
    Caps,
}

/// The timer surface: create (a real `timerfd`), check (the kernel's own
/// expiration count), watch (the subscription), and cancel (removal — gated).
#[derive(Subcommand, Debug)]
pub enum TimerAction {
    /// Arm a timer (`createTimer`).
    Create {
        /// Fire this long from now, in milliseconds.
        #[arg(long, default_value = "100")]
        after_ms: u64,
        /// The periodic interval in milliseconds (0 is one-shot).
        #[arg(long, default_value = "0")]
        interval_ms: u64,
        /// The label the subscription filter matches on.
        #[arg(long, default_value = "smart")]
        label: String,
        /// Which clock: realtime or monotonic.
        #[arg(long, default_value = "monotonic")]
        clock: String,
    },
    /// Did this timer fire since the last pull? (`checkTimer` — the kernel's
    /// own expiration count.)
    Check {
        /// The timer id.
        id: u64,
    },
    /// The session's timers: the rows this process armed plus the rows
    /// earlier processes left in the shared ledger (owned / gone).
    List,
    /// Disarm and retire a timer (`cancelTimer` — removal, gated).
    Cancel {
        /// The timer id.
        id: u64,
    },
}

/// The virtual-clock surface: software clocks, REAL by nature.
#[derive(Subcommand, Debug)]
pub enum VirtualAction {
    /// Create a virtual clock (`createVirtualClock`).
    Create {
        /// A debugging name.
        #[arg(long, default_value = "smart-virtual")]
        name: String,
        /// The rate multiplier (1.0 = real time, 0.0 = paused).
        #[arg(long, default_value = "1.0")]
        rate: f32,
    },
    /// Advance a virtual clock (`advanceVirtualClock`) and read its new time.
    Advance {
        /// The virtual clock's id.
        id: u64,
        /// How far to advance it, in milliseconds.
        millis: u64,
    },
}

/// The alarm surface, acquired through `alarms @19`: holding the returned
/// reference IS the authority — there is no per-method token.
#[derive(Subcommand, Debug)]
pub enum AlarmAction {
    /// The alarm list (`listAlarms`).
    List,
    /// The next alarm that will fire (`getNextAlarm`).
    Next,
    /// Create an alarm (`createAlarm`) in this process's own store.
    Create {
        /// The label.
        #[arg(long, default_value = "smart alarm")]
        label: String,
        /// Fire this long from now, in milliseconds.
        #[arg(long, default_value = "60000")]
        after_ms: u64,
        /// `daily`, `weekly`, `monthly` or one-shot (the default).
        #[arg(long, default_value = "once")]
        recurrence: String,
        /// How long a snooze defers it, in milliseconds.
        #[arg(long, default_value = "60000")]
        snooze_ms: u64,
    },
    /// Snooze an alarm (`snoozeAlarm`).
    Snooze {
        /// The alarm id.
        id: u64,
        /// How long to defer it, in milliseconds.
        millis: u64,
    },
    /// Delete an alarm (`deleteAlarm` — removal, gated).
    Delete {
        /// The alarm id.
        id: u64,
    },
}

/// Whether the caller accepted the host write (the REPL prompts; one-shot mode
/// passes the `--force` flag through) — the C1 gate, re-used. Creation arms
/// this process's own timers and its own alarm store, so only the REMOVAL
/// verbs are gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard {
    pub allowed: bool,
}

/// The clock domain a caller names, through the engine's own vocabulary.
fn clock_domain(token: &str) -> SmartResult<ClockDomainKind> {
    match token {
        "realtime" => Ok(ClockDomainKind::Realtime),
        "monotonic" => Ok(ClockDomainKind::Monotonic),
        // The contract's own catch-all: a domain this schema was frozen before
        // is named as custom rather than guessed at another host clock.
        "custom" => Ok(ClockDomainKind::Custom),
        other => Err(SmartError::invalid_argument(format!(
            "a clock domain is realtime or monotonic (the contract's own two), got {other:?}"
        ))),
    }
}

/// The recurrence a caller names, in the contract's own vocabulary.
fn recurrence_of(token: &str) -> Option<smart_lib::domains::clock::types::AlarmRecurrence> {
    use smart_lib::domains::clock::types::AlarmRecurrence;
    match token {
        "daily" => Some(AlarmRecurrence::Daily),
        "weekly" => Some(AlarmRecurrence::Weekly(vec![1, 2, 3, 4, 5])),
        "monthly" => Some(AlarmRecurrence::Monthly(1)),
        _ => None,
    }
}

/// The timer spec a `clock timer create` builds, through the engine's own type.
fn timer_spec(after_ms: u64, interval_ms: u64, label: &str, clock: &str) -> SmartResult<TimerSpec> {
    let domain = clock_domain(clock)?;
    Ok(TimerSpec {
        anchor: Timestamp::UNIX_EPOCH,
        interval: Duration::from_nanos(interval_ms.saturating_mul(1_000_000)),
        coalesce_missed: true,
        label: label.to_string(),
        clock_domain: domain,
        anchor_type: TimerAnchor::Relative(Duration::from_nanos(after_ms.saturating_mul(1_000_000))),
        group: String::new(),
        priority: 0,
    })
}

/// Evaluate the command's data half: the value the command produces, with no
/// rendering. This is what pipes consume.
pub fn evaluate(cmd: &ClockCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None | Some(ClockAction::Info) => render::info_value(backend),
        Some(ClockAction::Now) => {
            let nanos = backend.realtime_now().map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("realtime".into(), Value::String(nanos.as_nanos().to_string())),
                ("seconds".into(), Value::Number((nanos.as_nanos() / 1_000_000_000) as f64)),
            ]))
        }
        Some(ClockAction::Monotonic) => {
            let nanos = backend.monotonic_now().map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("monotonic".into(), Value::String(nanos.as_nanos().to_string())),
            ]))
        }
        Some(ClockAction::Uptime) => {
            let info = backend.info().map_err(SmartError::from)?;
            render::uptime_value(&info)
        }
        Some(ClockAction::Resolution) => {
            let domains = backend.list_clock_domains().map_err(SmartError::from)?;
            Ok(Value::List(
                domains
                    .iter()
                    .map(|domain| {
                        Value::Object(vec![
                            ("domain".into(), Value::String(domain.domain.as_token().to_string())),
                            ("name".into(), Value::String(domain.name.clone())),
                            (
                                "resolution".into(),
                                Value::String(domain.resolution.as_nanos().to_string()),
                            ),
                            ("available".into(), Value::Bool(domain.available)),
                        ])
                    })
                    .collect(),
            ))
        }
        Some(ClockAction::Sync) => {
            let info = backend.get_clock_sync().map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("synced".into(), Value::Bool(info.synced)),
                ("source".into(), Value::String(info.source.clone())),
                ("stratum".into(), Value::Number(f64::from(info.stratum))),
                ("last_sync".into(), Value::String(info.last_sync.as_nanos().to_string())),
                ("offset".into(), Value::String(info.offset.as_nanos().to_string())),
                ("evidence".into(), Value::String(SyncEvidenceSurface::sync_evidence(backend))),
            ]))
        }
        Some(ClockAction::Security) => {
            let security = backend.security_state();
            Ok(Value::Object(vec![
                ("tampered".into(), Value::Bool(security.tampered)),
                ("secure_source".into(), Value::Bool(security.secure_source)),
                ("hardware_rng".into(), Value::Bool(security.hardware_rng)),
            ]))
        }
        Some(ClockAction::Domains) => {
            let domains = backend.list_clock_domains().map_err(SmartError::from)?;
            Ok(Value::List(
                domains
                    .iter()
                    .map(render::domain_value)
                    .collect(),
            ))
        }
        Some(ClockAction::Quality { source }) => {
            let metrics = backend.get_clock_quality(source).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("source".into(), Value::String(metrics.source_name.clone())),
                ("accuracy".into(), Value::String(metrics.accuracy.as_nanos().to_string())),
                ("stability".into(), Value::String(metrics.stability.as_nanos().to_string())),
                ("reliability".into(), Value::Number(f64::from(metrics.reliability))),
                ("samples".into(), Value::Number(f64::from(metrics.sample_count))),
                (
                    "measured_at".into(),
                    Value::String(metrics.measured_at.as_nanos().to_string()),
                ),
            ]))
        }
        Some(ClockAction::Drift { a, b }) => {
            let drift = backend
                .get_clock_drift(clock_domain(a)?, clock_domain(b)?)
                .map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("drift".into(), Value::String(drift.drift.as_nanos().to_string())),
                ("accuracy".into(), Value::String(drift.accuracy.as_nanos().to_string())),
                (
                    "measured_at".into(),
                    Value::String(drift.measured_at.as_nanos().to_string()),
                ),
            ]))
        }
        Some(ClockAction::Sleep { millis }) => {
            backend
                .sleep(Duration::from_nanos(millis.saturating_mul(1_000_000)))
                .map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("sleep".into())),
                ("millis".into(), Value::Number(*millis as f64)),
            ]))
        }
        Some(ClockAction::Timer { action }) => render::timer_value(backend, action, guard),
        Some(ClockAction::SleepCancellable {
            millis,
            cancel_after,
        }) => {
            let handle = backend
                .sleep_cancellable(Duration::from_nanos(millis.saturating_mul(1_000_000)))
                .map_err(SmartError::from)?;
            if let Some(after) = cancel_after {
                guard_host_write(guard, &format!("clock sleep {millis} --cancel-after {after}"))?;
                backend.cancel_sleep(handle).map_err(SmartError::from)?;
                return Ok(Value::Object(vec![
                    ("verb".into(), Value::String("sleep-cancellable".into())),
                    ("millis".into(), Value::Number(*millis as f64)),
                    ("id".into(), Value::Number(handle.id as f64)),
                    ("cancelled".into(), Value::Bool(true)),
                ]));
            }
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("sleep-cancellable".into())),
                ("millis".into(), Value::Number(*millis as f64)),
                ("id".into(), Value::Number(handle.id as f64)),
                ("cancelled".into(), Value::Bool(false)),
            ]))
        }
        Some(ClockAction::Virtual { action }) => render::virtual_value(backend, action),
        Some(ClockAction::Alarm { action }) => render::alarm_value(backend, action, guard),
        Some(ClockAction::Watch { count, interval }) => render::watch_value(backend, *count, *interval),
        Some(ClockAction::Caps) => Ok(render::caps_value(backend)),
    }
}

/// Run the command: evaluate, render in the requested format.
pub fn run(cmd: &ClockCmd, format: &OutputFormat, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    let value = evaluate(cmd, backend, guard)?;
    if *format != OutputFormat::Raw {
        println!("{}", render::render_action(cmd, &value, *format));
    }
    Ok(value)
}

/// Run with the session's timer ledger visible.
///
/// This is the path both the REPL and the one-shot binary take: a `timer
/// create` is recorded in the session (and, for a persistent session, in the
/// shared state file the next process reads), a `timer cancel` retires the
/// row, and `timer list` names the session's LIVE timers — each armed through
/// the engine, each checkable here — plus the ledger rows earlier processes
/// left, each with the measured ownership (a live sibling owns its fd; a dead
/// owner's fd died with it). The engine itself offers no list-timers verb, so
/// the shell's session ledger IS the surface, and it never claims a firing it
/// cannot read.
pub fn run_session(
    cmd: &ClockCmd,
    format: &OutputFormat,
    backend: &Backend,
    guard: WriteGuard,
    session: &mut crate::session::ClockSession,
) -> SmartResult<Value> {
    let value = evaluate_session(cmd, backend, guard, session)?;
    if *format != OutputFormat::Raw {
        println!("{}", render::render_action(cmd, &value, *format));
    }
    Ok(value)
}

/// The data half of [`run_session`]: the ledger-aware evaluation, which is
/// what pipes consume.
pub fn evaluate_session(
    cmd: &ClockCmd,
    backend: &Backend,
    guard: WriteGuard,
    session: &mut crate::session::ClockSession,
) -> SmartResult<Value> {
    // `timer list` is the session's own surface: the engine has no
    // list-timers verb, so the ledger answers it.
    if matches!(
        &cmd.action,
        Some(ClockAction::Timer { action: TimerAction::List })
    ) {
        return Ok(render::timer_list_value(session));
    }
    let value = evaluate(cmd, backend, guard)?;
    match &cmd.action {
        Some(ClockAction::Timer {
            action: TimerAction::Create { after_ms, interval_ms, label, clock },
        }) => {
            // The engine's own answer carries the id it minted; the spec is
            // the one this command built. The row is recorded in the session
            // (and shared through the state file when the session persists) —
            // the delay and the interval both travel, so a later process reads
            // the spec this one actually armed.
            let id = value.field("id").as_f64().unwrap_or(0.0) as u64;
            session.record(crate::session::TimerRecord {
                pid: std::process::id(),
                id,
                label: label.clone(),
                after_ms: *after_ms,
                interval_ms: *interval_ms,
                clock: clock.clone(),
                armed_at_nanos: backend
                    .realtime_now()
                    .map(|t| t.as_nanos() as u128)
                    .unwrap_or(0),
                session: session.session().to_string(),
            });
        }
        Some(ClockAction::Timer {
            action: TimerAction::Cancel { id },
        }) => session.retire(*id),
        _ => {}
    }
    Ok(value)
}

/// The host write this command performs, when it performs one: the verb and
/// the target the confirmation prompt shows. Arming a timer or creating an
/// alarm is this process's OWN state (a timerfd it owns, its own alarm store),
/// so only the REMOVAL half is gated — exactly the C1 shape.
pub fn write_intent(cmd: &ClockCmd) -> Option<(String, String)> {
    use ClockAction as C;
    Some(match &cmd.action {
        Some(C::Timer { action: TimerAction::Cancel { id } }) => {
            ("cancel timer".into(), format!("#{id}"))
        }
        Some(C::Alarm { action: AlarmAction::Delete { id } }) => {
            ("delete alarm".into(), format!("#{id}"))
        }
        Some(C::SleepCancellable { millis, cancel_after: Some(after) }) => {
            ("cancel".into(), format!("the {millis} ms sleep after {after} ms"))
        }
        _ => return None,
    })
}

pub mod render;

/// The host-write gate (law #8, the C1 shape): a REMOVAL verb proceeds only
/// when the caller accepted the write.
fn guard_host_write(guard: WriteGuard, verb: &str) -> SmartResult<()> {
    if guard.allowed {
        return Ok(());
    }
    Err(SmartError::not_supported(format!(
        "refusing to run `{verb}`: it CHANGES state. The interactive `(y/N)` prompt asks with the target's context \
         beside it; a piped `y` line answers it, and `--force` accepts the write \
         explicitly (the live bring-back proof is founder-run)."
    )))
}

/// The clock selector tree, rendered from the engine's owned types — the same
/// values the command surface renders, so a selector and a command can never
/// disagree. `@clock/info`, `@clock/sync`, `@clock/caps` resolve from here.
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let info = backend.info().map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("info".into(), render::clock_info_value(&info)),
        ("caps".into(), render::caps_value(backend)),
        (
            "domains".into(),
            Value::List(
                info.domains
                    .iter()
                    .map(render::domain_value)
                    .collect(),
            ),
        ),
    ]))
}

/// The alarm this process's own store holds, as a record.
pub(crate) fn alarm_record(alarm: &Alarm) -> Value {
    Value::Object(vec![
        ("id".into(), Value::Number(alarm.id.get() as f64)),
        ("label".into(), Value::String(alarm.label.clone())),
        (
            "fire_time".into(),
            Value::String(alarm.fire_time.as_nanos().to_string()),
        ),
        (
            "recurrence".into(),
            match &alarm.recurrence {
                Some(rule) => Value::String(format!("{rule:?}")),
                None => Value::String("once".to_string()),
            },
        ),
        (
            "snooze".into(),
            Value::String(alarm.snooze_duration.as_nanos().to_string()),
        ),
        ("enabled".into(), Value::Bool(alarm.enabled)),
        (
            "holiday".into(),
            Value::String(format!("{:?}", alarm.holiday_behavior.mode)),
        ),
    ])
}

/// The virtual clock config a caller names.
pub(crate) fn virtual_config(name: &str, rate: f32) -> VirtualClockConfig {
    VirtualClockConfig {
        initial_time: Timestamp::UNIX_EPOCH,
        domain: ClockDomainKind::Monotonic,
        rate,
        name: name.to_string(),
    }
}

/// One holiday behaviour: the neutral default (no holiday rule is applied when
/// a caller says nothing, and saying "skip" would be a policy guess).
pub(crate) fn neutral_holiday() -> HolidayBehavior {
    HolidayBehavior::default()
}

/// Re-export for the shell and main: the clap shape.
pub use self::{AlarmAction as AlarmSub, ClockAction as Action, ClockCmd as Cmd, TimerAction as TimerSub, VirtualAction as VirtualSub};

/// One alarm id.
pub(crate) fn alarm_id(value: u64) -> AlarmId {
    AlarmId::new(value)
}
