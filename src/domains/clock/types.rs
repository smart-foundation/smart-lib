//! Owned value types the **clock** domain speaks (the types a caller actually
//! touches), plus their conversions to and from the frozen `clock.capnp`.
//!
//! Law #7: the capnp state structs are generated; this module is the
//! *hand-written* mirror used at the domain's public/backend seam. It is
//! ergonomic (owned, `Copy` where the contract allows) for the methods this
//! milestone implements, and a compact faithful echo for the rest.
//!
//! ## Time units are the contract's own
//!
//! Every instant here is [`crate::core::values::Timestamp`] (nanoseconds since
//! the Unix epoch) or [`crate::core::values::Duration`] (nanoseconds) — the
//! contract's two time units and no third. A backend that reports a resolution
//! reports it in the same unit the caller already understands.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::core::ids::{AlarmId, TimerId};
use crate::core::values::{Duration, Timestamp};
use crate::schema::{calendar_capnp, clock_capnp, types_capnp};

/// Pagination, mirroring `T.Pagination` (`offset`, `limit`).
///
/// The contract's rule (conformance §2.3): `limit == 0` means "one", never
/// "all"; `offset + limit` may never exceed `total`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    /// Where the page starts (0-based).
    pub offset: u32,
    /// How many entries the caller will take.
    pub limit: u32,
}

impl Pagination {
    /// Effective limit under the contract rule: 0 means "one".
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 {
            1
        } else {
            self.limit
        }
    }
}

/// A one-page window over an unbounded list, with the stable `total` the
/// contract promises across two identical calls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    /// The entries this page carries.
    pub items: Vec<T>,
    /// The total the caller may page through (stable across identical calls).
    pub total: u64,
}

/// The clock domains the contract names (`ClockDomain`), as an enum in Rust so
/// the three are distinct typed values the compiler cannot collapse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClockDomainKind {
    /// Wall-clock time (`CLOCK_REALTIME`).
    Realtime,
    /// Boot-relative monotonic time (`CLOCK_MONOTONIC`).
    Monotonic,
    /// A clock this schema was frozen before.
    Custom,
}

impl ClockDomainKind {
    /// The contract ordinal.
    pub const fn ordinal(self) -> u16 {
        match self {
            ClockDomainKind::Realtime => 0,
            ClockDomainKind::Monotonic => 1,
            ClockDomainKind::Custom => 2,
        }
    }

    /// Read the enum out of the generated `ClockDomain`.
    pub fn from_generated(domain: clock_capnp::ClockDomain) -> Self {
        match domain {
            clock_capnp::ClockDomain::Realtime => ClockDomainKind::Realtime,
            clock_capnp::ClockDomain::Monotonic => ClockDomainKind::Monotonic,
            clock_capnp::ClockDomain::Custom => ClockDomainKind::Custom,
        }
    }

    /// The host clock id this domain maps to on Linux (a `clockid_t`).
    pub const fn host_clock_id(self) -> i32 {
        match self {
            ClockDomainKind::Realtime => 0,  // CLOCK_REALTIME
            ClockDomainKind::Monotonic => 1, // CLOCK_MONOTONIC
            ClockDomainKind::Custom => 1,    // CLOCK_MONOTONIC — the honest fallback
        }
    }

    /// The token the CLI renders.
    pub const fn as_token(self) -> &'static str {
        match self {
            ClockDomainKind::Realtime => "realtime",
            ClockDomainKind::Monotonic => "monotonic",
            ClockDomainKind::Custom => "custom",
        }
    }

    /// The open token for a backend that names a domain the enum does not
    /// carry: unknown tolerance, never a guessed domain.
    pub fn from_token(token: &str) -> Self {
        match token {
            "realtime" => ClockDomainKind::Realtime,
            "monotonic" => ClockDomainKind::Monotonic,
            _ => ClockDomainKind::Custom,
        }
    }

    /// Write into the generated `ClockDomain` field.
    pub fn to_generated(self) -> clock_capnp::ClockDomain {
        match self {
            ClockDomainKind::Realtime => clock_capnp::ClockDomain::Realtime,
            ClockDomainKind::Monotonic => clock_capnp::ClockDomain::Monotonic,
            ClockDomainKind::Custom => clock_capnp::ClockDomain::Custom,
        }
    }
}

impl fmt::Display for ClockDomainKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_token())
    }
}

/// The measured state of one clock source (`ClockInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct ClockInfo {
    /// The monotonic reading at the instant this info was taken.
    pub monotonic: Timestamp,
    /// The wall-clock reading at the instant this info was taken.
    pub realtime: Timestamp,
    /// The clock's measured resolution (nanoseconds); zero when the host does
    /// not answer, never a fabricated precision.
    pub resolution: Duration,
    /// The backend's open token for the source ("tsc", "hpet", ...).
    pub source_name: String,
    /// Time since the last boot (the monotonic base expressed as a timestamp).
    pub boot_time: Timestamp,
    /// Measured NTP/PTP synchronization state.
    pub sync: ClockSyncInfo,
    /// Measured tamper/secure-source state.
    pub security: ClockSecurityInfo,
    /// The clock domains this host can serve.
    pub domains: Vec<ClockDomainInfo>,
}

impl ClockInfo {
    /// Write into a generated `ClockInfo` builder.
    pub fn write_to(&self, builder: &mut clock_capnp::clock_info::Builder<'_>) {
        builder.reborrow().init_monotonic().set_nanoseconds(self.monotonic.as_nanos());
        builder.reborrow().init_realtime().set_nanoseconds(self.realtime.as_nanos());
        builder.reborrow().init_resolution().set_nanoseconds(self.resolution.as_nanos());
        builder.set_source_name(&self.source_name);
        builder.reborrow().init_boot_time().set_nanoseconds(self.boot_time.as_nanos());
        self.sync.write_to(&mut builder.reborrow().init_sync());
        self.security.write_to(&mut builder.reborrow().init_security());
        let mut list = builder.reborrow().init_domains(self.domains.len() as u32);
        for (i, domain) in self.domains.iter().enumerate() {
            domain.write_to(&mut list.reborrow().get(i as u32));
        }
    }
}

/// One available clock domain (`ClockDomainInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockDomainInfo {
    /// The domain itself.
    pub domain: ClockDomainKind,
    /// A human-readable name (the host's own word for it).
    pub name: String,
    /// The measured timer resolution for this domain.
    pub resolution: Duration,
    /// Whether this domain is available on this host (measured).
    pub available: bool,
}

impl ClockDomainInfo {
    /// Write into a generated `ClockDomainInfo` builder.
    pub fn write_to(&self, builder: &mut clock_capnp::clock_domain_info::Builder<'_>) {
        builder.set_domain(self.domain.to_generated());
        builder.set_name(&self.name);
        builder.reborrow().init_resolution().set_nanoseconds(self.resolution.as_nanos());
        builder.set_available(self.available);
    }

    /// Read from a generated `ClockDomainInfo` reader.
    pub fn from_reader(reader: &clock_capnp::clock_domain_info::Reader<'_>) -> Self {
        ClockDomainInfo {
            domain: ClockDomainKind::from_generated(reader.get_domain().unwrap_or(clock_capnp::ClockDomain::Realtime)),
            name: text_field(reader.get_name()),
            resolution: reader
                .get_resolution()
                .map(|r| Duration::from_nanos(r.get_nanoseconds()))
                .unwrap_or_default(),
            available: reader.get_available(),
        }
    }
}

/// Measured NTP/PTP synchronization status (`ClockSyncInfo`).
///
/// The law this type exists to hold: `synced` is **measured** — a backend that
/// cannot see a synchronization daemon answers `synced: false` with the reason
/// the live behaviour repeats, never `synced: true` because a clock exists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClockSyncInfo {
    /// Whether this host's clock is currently synchronized (measured).
    pub synced: bool,
    /// The sync source as an open token ("chrony", "ntpd", "ptp-local", ...).
    pub source: String,
    /// NTP stratum (0 when the host does not report one — never invented).
    pub stratum: u32,
    /// When the last synchronization happened (0 when unreported).
    pub last_sync: Timestamp,
    /// Current offset from reference time (0 when unreported).
    pub offset: Duration,
}

impl ClockSyncInfo {
    /// Write into a generated `ClockSyncInfo` builder.
    pub fn write_to(&self, builder: &mut clock_capnp::clock_sync_info::Builder<'_>) {
        builder.set_synced(self.synced);
        builder.set_source(&self.source);
        builder.set_stratum(self.stratum);
        builder.reborrow().init_last_sync().set_nanoseconds(self.last_sync.as_nanos());
        builder.reborrow().init_offset().set_nanoseconds(self.offset.as_nanos());
    }

    /// Read from a generated `ClockSyncInfo` reader.
    pub fn from_reader(reader: &clock_capnp::clock_sync_info::Reader<'_>) -> Self {
        ClockSyncInfo {
            synced: reader.get_synced(),
            source: text_field(reader.get_source()),
            stratum: reader.get_stratum(),
            last_sync: reader
                .get_last_sync()
                .map(|t| Timestamp::from_nanos(t.get_nanoseconds()))
                .unwrap_or_default(),
            offset: reader
                .get_offset()
                .map(|d| Duration::from_nanos(d.get_nanoseconds()))
                .unwrap_or_default(),
        }
    }
}

/// Measured clock tamper/secure-source state (`ClockSecurityInfo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockSecurityInfo {
    /// Whether tampering has been detected (measured, never assumed clean).
    pub tampered: bool,
    /// Whether a hardware-secured time source is available.
    pub secure_source: bool,
    /// Whether a hardware RNG is available for time jitter.
    pub hardware_rng: bool,
}

impl ClockSecurityInfo {
    /// Write into a generated `ClockSecurityInfo` builder.
    pub fn write_to(&self, builder: &mut clock_capnp::clock_security_info::Builder<'_>) {
        builder.set_tampered(self.tampered);
        builder.set_secure_source(self.secure_source);
        builder.set_hardware_rng(self.hardware_rng);
    }
}

/// What a timer's anchor means (`TimerAnchor`). The `custom` arm is the
/// contract's own forward-compatibility outlet: an anchor kind this schema
/// does not name never silently means "absolute".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerAnchor {
    /// Fire at this absolute timestamp.
    Absolute(Timestamp),
    /// Fire this long from now.
    Relative(Duration),
    /// Fire at zero (the old implicit behaviour).
    Zero,
    /// A kind this build does not name; the payload stays opaque.
    Custom,
}

impl Default for TimerAnchor {
    /// The neutral anchor: zero, the contract's documented default meaning.
    fn default() -> Self {
        TimerAnchor::Zero
    }
}

/// A timer specification (`TimerSpec`).
#[derive(Clone, Debug, PartialEq)]
pub struct TimerSpec {
    /// The anchor timestamp (its meaning comes from [`TimerSpec::anchor_type`]).
    pub anchor: Timestamp,
    /// Interval for periodic timers; zero for one-shot.
    pub interval: Duration,
    /// Whether missed firings are coalesced (the contract default is true).
    pub coalesce_missed: bool,
    /// A free-text label the subscription filter matches on.
    pub label: String,
    /// Which clock the anchor is expressed in.
    pub clock_domain: ClockDomainKind,
    /// Explicit anchor semantics (never inferred when absent).
    pub anchor_type: TimerAnchor,
    /// Timer group membership (for bulk operations).
    pub group: String,
    /// Timer priority (higher fires first).
    pub priority: u32,
}

impl TimerSpec {
    /// A one-shot relative timer: the common case, stated explicitly rather
    /// than inferred from a zero anchor.
    pub fn one_shot_relative(after: Duration) -> Self {
        TimerSpec {
            anchor: Timestamp::UNIX_EPOCH,
            interval: Duration::ZERO,
            coalesce_missed: true,
            label: String::new(),
            clock_domain: ClockDomainKind::Realtime,
            anchor_type: TimerAnchor::Relative(after),
            group: String::new(),
            priority: 0,
        }
    }

    /// Write into a generated `TimerSpec` builder.
    pub fn write_to(&self, builder: &mut clock_capnp::timer_spec::Builder<'_>) {
        builder.reborrow().init_anchor().set_nanoseconds(self.anchor.as_nanos());
        builder.reborrow().init_interval().set_nanoseconds(self.interval.as_nanos());
        builder.set_coalesce_missed(self.coalesce_missed);
        builder.set_label(&self.label);
        builder.set_clock_domain(self.clock_domain.to_generated());
        {
            let mut anchor = builder.reborrow().init_anchor_type();
            match self.anchor_type {
                TimerAnchor::Absolute(t) => {
                    anchor.reborrow().init_absolute().set_nanoseconds(t.as_nanos());
                }
                TimerAnchor::Relative(d) => {
                    anchor.reborrow().init_relative().set_nanoseconds(d.as_nanos());
                }
                TimerAnchor::Zero => {
                    anchor.set_zero(());
                }
                TimerAnchor::Custom => {
                    // The contract's forward-compatibility slot: a kind this
                    // build does not name travels as an opaque (empty) payload
                    // rather than collapsing onto `absolute`.
                    anchor.set_custom(&[]);
                }
            }
        }
        builder.set_group(&self.group);
        builder.set_priority(self.priority);
    }

    /// Read from a generated `TimerSpec` reader.
    pub fn from_reader(reader: &clock_capnp::timer_spec::Reader<'_>) -> Self {
        let anchor_type = match reader.get_anchor_type().ok().map(|a| a.which()) {
            Some(Ok(clock_capnp::timer_anchor::WhichReader::Absolute(Ok(a)))) => {
                TimerAnchor::Absolute(Timestamp::from_nanos(a.get_nanoseconds()))
            }
            Some(Ok(clock_capnp::timer_anchor::WhichReader::Relative(Ok(r)))) => {
                TimerAnchor::Relative(Duration::from_nanos(r.get_nanoseconds()))
            }
            Some(Ok(clock_capnp::timer_anchor::WhichReader::Zero(()))) => TimerAnchor::Zero,
            _ => TimerAnchor::Custom,
        };
        TimerSpec {
            anchor: reader
                .get_anchor()
                .map(|t| Timestamp::from_nanos(t.get_nanoseconds()))
                .unwrap_or_default(),
            interval: reader
                .get_interval()
                .map(|d| Duration::from_nanos(d.get_nanoseconds()))
                .unwrap_or_default(),
            coalesce_missed: reader.get_coalesce_missed(),
            label: text_field(reader.get_label()),
            clock_domain: ClockDomainKind::from_generated(
                reader.get_clock_domain().unwrap_or(clock_capnp::ClockDomain::Realtime),
            ),
            anchor_type,
            group: text_field(reader.get_group()),
            priority: reader.get_priority(),
        }
    }

    /// The instant this timer first fires, on the domain's own clock, resolved
    /// against `now` (a real reading, so a relative anchor is measured against
    /// the host's clock and not against a caller's guess).
    pub fn resolve_first_fire(&self, now: Timestamp) -> Timestamp {
        match self.anchor_type {
            TimerAnchor::Absolute(t) => t,
            TimerAnchor::Relative(d) => {
                Timestamp::from_nanos(now.as_nanos().saturating_add(d.as_nanos()))
            }
            // Zero means "from now", so a zero anchor fires immediately — the
            // contract's own documented meaning, stated rather than guessed.
            TimerAnchor::Zero => Timestamp::UNIX_EPOCH,
            TimerAnchor::Custom => now,
        }
    }

    /// Whether this timer repeats (`interval != 0`).
    pub const fn is_periodic(&self) -> bool {
        self.interval.as_nanos() != 0
    }
}

/// An armed timer's handle (`TimerHandle`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimerHandle {
    /// The contract's opaque timer id.
    pub id: TimerId,
    /// The label the spec carried (empty when the spec did).
    pub label: String,
}

impl TimerHandle {
    /// Write into a generated `TimerHandle` builder.
    pub fn write_to(&self, builder: &mut clock_capnp::timer_handle::Builder<'_>) {
        builder.reborrow().init_id().set_id(self.id.get());
        builder.set_label(&self.label);
    }

    /// Read from a generated `TimerHandle` reader.
    pub fn from_reader(reader: &clock_capnp::timer_handle::Reader<'_>) -> Self {
        TimerHandle {
            id: TimerId::new(reader.get_id().map(|i| i.get_id()).unwrap_or(0)),
            label: text_field(reader.get_label()),
        }
    }
}

/// A subscription filter (`TimerFilter`): an empty label list means every
/// timer, never none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimerFilter {
    /// Labels to match; empty matches all.
    pub labels: Vec<String>,
}

/// One timer firing (`TimerEvent`).
#[derive(Clone, Debug, PartialEq)]
pub struct TimerEvent {
    /// The timer that fired.
    pub timer: TimerHandle,
    /// When it fired.
    pub fired_at: Timestamp,
    /// How many times it fired since the last pull (the kernel's own count for
    /// a timerfd, never a fabricated 1).
    pub count: u64,
}

impl TimerEvent {
    /// Write into a generated `TimerEvent` builder.
    pub fn write_to(&self, builder: &mut clock_capnp::timer_event::Builder<'_>) {
        self.timer.write_to(&mut builder.reborrow().init_timer());
        builder.reborrow().init_fired_at().set_nanoseconds(self.fired_at.as_nanos());
        builder.set_count(self.count);
    }
}

/// An opaque subscription handle, minted by `subscribe_timers`. The pull model
/// is the contract's: subscribing mints a token, pulling reads the current
/// value under the caller's stated bound, and the token is retired on cancel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

/// A cancellable sleep handle (`SleepHandle`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SleepHandle {
    /// The opaque id the caller cancels with.
    pub id: u64,
}

/// A drift measurement between two clock domains (`ClockDriftInfo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockDriftInfo {
    /// The measured drift.
    pub drift: Duration,
    /// When the drift was measured.
    pub measured_at: Timestamp,
    /// The accuracy of the measurement itself.
    pub accuracy: Duration,
}

/// A virtual clock configuration (`VirtualClockConfig`) — the testing clock.
#[derive(Clone, Debug, PartialEq)]
pub struct VirtualClockConfig {
    /// Initial time for the virtual clock.
    pub initial_time: Timestamp,
    /// The domain the virtual clock is expressed in.
    pub domain: ClockDomainKind,
    /// Rate multiplier (1.0 = real time, 0.0 = paused).
    pub rate: f32,
    /// A debugging name.
    pub name: String,
}

/// A virtual clock handle (`VirtualClockHandle`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirtualClockHandle {
    /// The opaque id.
    pub id: u64,
    /// The name the config carried.
    pub name: String,
}

/// Clock quality metrics for one source (`ClockQualityMetrics`).
#[derive(Clone, Debug, PartialEq)]
pub struct ClockQualityMetrics {
    /// The source these metrics describe.
    pub source_name: String,
    /// Accuracy (maximum expected error).
    pub accuracy: Duration,
    /// Stability (jitter/variance).
    pub stability: Duration,
    /// Reliability 0.0-1.0 (proportion of successful syncs).
    pub reliability: f32,
    /// When these metrics were last measured.
    pub measured_at: Timestamp,
    /// How many samples were used.
    pub sample_count: u32,
}

/// Timer persistence configuration (`TimerPersistence`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimerPersistence {
    /// The persisted name.
    pub name: String,
    /// Whether the timer re-fires after a process restart.
    pub re_fire_on_restart: bool,
    /// Maximum re-fires (0 = unlimited).
    pub max_re_fires: u32,
}

// ── Alarms (`AlarmService`) ─────────────────────────────────────────────

/// One alarm (`Alarm`).
#[derive(Clone, Debug, PartialEq)]
pub struct Alarm {
    /// The alarm id.
    pub id: AlarmId,
    /// A label.
    pub label: String,
    /// When it fires.
    pub fire_time: Timestamp,
    /// The recurrence rule (`None` = one-shot).
    pub recurrence: Option<AlarmRecurrence>,
    /// How long a snooze defers it.
    pub snooze_duration: Duration,
    /// Whether it is enabled.
    pub enabled: bool,
    /// Holiday behaviour.
    pub holiday_behavior: HolidayBehavior,
}

impl Alarm {
    /// Write into a generated `Alarm` builder.
    pub fn write_to(&self, builder: &mut clock_capnp::alarm::Builder<'_>) {
        builder.reborrow().init_id().set_id(self.id.get());
        builder.set_label(&self.label);
        builder.reborrow().init_fire_time().set_nanoseconds(self.fire_time.as_nanos());
        if let Some(rule) = &self.recurrence {
            let mut recurrence = builder.reborrow().init_recurrence();
            match rule {
                AlarmRecurrence::Daily => {
                    recurrence.set_daily(());
                }
                AlarmRecurrence::Weekly(days) => {
                    let weekly = recurrence.init_weekly();
                    let mut out = weekly.init_days_of_week(days.len() as u32);
                    for (i, day) in days.iter().enumerate() {
                        out.set(i as u32, calendar_day_of_week(*day));
                    }
                }
                AlarmRecurrence::Monthly(day) => {
                    recurrence.reborrow().init_monthly().set_day_of_month(*day);
                }
                AlarmRecurrence::Cron(expr) => {
                    recurrence.set_cron(expr);
                }
                AlarmRecurrence::Custom => {}
            }
        }
        builder.reborrow().init_snooze_duration().set_nanoseconds(self.snooze_duration.as_nanos());
        builder.set_enabled(self.enabled);
        self.holiday_behavior.write_to(&mut builder.reborrow().init_holiday_behavior());
    }
}

/// The recurrence rule of an alarm (`AlarmRecurrence`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlarmRecurrence {
    /// Daily at `fireTime`.
    Daily,
    /// Weekly on the named days.
    Weekly(Vec<u8>),
    /// Monthly on a day of the month.
    Monthly(u8),
    /// A cron-like expression (interpreted by the backend; open token).
    Cron(String),
    /// A rule this build does not name.
    Custom,
}

/// Holiday behaviour of an alarm (`HolidayBehavior`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HolidayBehavior {
    /// What to do when the alarm falls on a holiday.
    pub mode: HolidayMode,
    /// Alternative fire time when the mode is `adjust`.
    pub adjusted_time: Timestamp,
    /// Holiday calendar ids to check.
    pub holiday_calendars: Vec<String>,
}

impl HolidayBehavior {
    /// Write into a generated `HolidayBehavior` builder.
    pub fn write_to(&self, builder: &mut clock_capnp::holiday_behavior::Builder<'_>) {
        builder.set_mode(self.mode.to_generated());
        builder.reborrow().init_adjusted_time().set_nanoseconds(self.adjusted_time.as_nanos());
        let mut list = builder.reborrow().init_holiday_calendars(self.holiday_calendars.len() as u32);
        for (i, calendar) in self.holiday_calendars.iter().enumerate() {
            list.reborrow().set(i as u32, calendar);
        }
    }
}

/// What an alarm does on a holiday (`HolidayMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HolidayMode {
    /// Fire normally (the neutral default: no holiday rule is applied when a
    /// caller says nothing, and saying "skip" would be a policy guess).
    #[default]
    Fire,
    /// Skip it.
    Skip,
    /// Move it to `adjustedTime`.
    Adjust,
    /// Snooze to the next non-holiday.
    Snooze,
    /// A mode this build does not name.
    Custom,
}

impl HolidayMode {
    /// Write into the generated `HolidayMode` field.
    pub fn to_generated(self) -> clock_capnp::HolidayMode {
        match self {
            HolidayMode::Fire => clock_capnp::HolidayMode::Fire,
            HolidayMode::Skip => clock_capnp::HolidayMode::Skip,
            HolidayMode::Adjust => clock_capnp::HolidayMode::Adjust,
            HolidayMode::Snooze => clock_capnp::HolidayMode::Snooze,
            HolidayMode::Custom => clock_capnp::HolidayMode::Custom,
        }
    }
}

/// The `calendar.capnp` `DayOfWeek` value for the 0..6 day index the contract
/// names (0 = Sunday, the calendar type's own numbering), so a backend and the
/// contract agree on which day is which.
pub fn calendar_day_of_week(day: u8) -> calendar_capnp::DayOfWeek {
    use calendar_capnp::DayOfWeek as D;
    match day {
        0 => D::Sunday,
        1 => D::Monday,
        2 => D::Tuesday,
        3 => D::Wednesday,
        4 => D::Thursday,
        5 => D::Friday,
        6 => D::Saturday,
        // An unnamed day is `Custom`, the enum's own open member — never a
        // guessed weekday.
        _ => D::Custom,
    }
}

/// Read a `Text` field into an owned string (absent/invalid UTF-8 is empty —
/// the same equivalence the writers emit, so a round-trip stays total).
pub(crate) fn text_field(reader: capnp::Result<capnp::text::Reader<'_>>) -> String {
    match reader {
        Ok(text) => core::str::from_utf8(text.as_bytes()).unwrap_or("").to_string(),
        Err(_) => String::new(),
    }
}

#[allow(dead_code)]
fn assert_generated_types_linked() {
    // Naming (not constructing) the generated types proves the domain feature
    // compiled the clock schema; the private tuple fields stop construction.
    let _: Option<clock_capnp::clock_info::Owned> = None;
    let _: Option<types_capnp::timestamp::Owned> = None;
}
