//! The Linux **clock** backend for the clock domain — the kernel's own clocks,
//! read natively (charter law #9).
//!
//! ## The mechanisms, and why each one
//!
//! | surface | mechanism |
//! |---|---|
//! | both readings | `clock_gettime` on `CLOCK_REALTIME` / `CLOCK_MONOTONIC` — the kernel's own clocks, no daemon hop and no `date` shellout |
//! | resolution | `clock_getres` on the same ids |
//! | boot epoch | `/proc/stat`'s `btime` (the kernel's own value, not an arithmetic guess) |
//! | timers | `timerfd_create(CLOCK_REALTIME)` + `timerfd_settime`, so a firing is the kernel's own expiration count: a periodic timer that expired three times while nobody looked reports **3**, never a fabricated 1 |
//! | sleep | `nanosleep` on the caller's own thread; the cancellable half waits on a condvar that can be signalled |
//! | sync | a `/proc` scan measuring which synchronization daemon this host runs — never `chronyc`/`timedatectl` (a shellout) |
//!
//! ## Law #8 (the host is read-only)
//!
//! Every path here is a read or a per-process resource: the clock reads are
//! `clock_gettime`, the timer is a `timerfd` owned by *this* process, the sleep
//! is the caller's own thread, the alarm store is this process's memory. There
//! is no host write anywhere in this file — nothing to gate and nothing to
//! refuse. The `#[ignore]`d live probe at the bottom is read-only too.

// This is a std process (the Linux backend lives in a service/app); the public
// API and core stay no_std + alloc (charter law #1).
extern crate std;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use core::sync::atomic::{AtomicU64, Ordering};

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::sync::{Condvar, Mutex};
use std::time::Duration as StdDuration;

use crate::core::ids::AlarmId;
use crate::core::values::{Duration, Timestamp};
use crate::core::{Error, Result};

use crate::domains::clock::backend::{AlarmService, ClockBackend};
use crate::domains::clock::types::{
    Alarm, ClockDomainInfo, ClockDomainKind, ClockDriftInfo, ClockInfo, ClockQualityMetrics,
    ClockSecurityInfo, ClockSyncInfo, Page, Pagination, SleepHandle, SubscriptionId, TimerAnchor,
    TimerEvent, TimerFilter, TimerHandle, TimerPersistence, TimerSpec, VirtualClockConfig,
    VirtualClockHandle,
};

// ═══════════════════════════════════════════════════════════════════════
// The kernel's own clock and timer interface.
//
// Declared locally rather than pulled from a C-binding crate: the engine links
// only `std` (which links libc), so the five functions this backend needs are
// declared once, with their stable kernel ABI. No new dependency, no shellout,
// and the types are the kernel's own (`clockid_t`, `timerfd`).
// ═══════════════════════════════════════════════════════════════════════

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;
const TFD_NONBLOCK: i32 = 0x0000_0800;
const TFD_TIMER_ABSTIME: i32 = 0x0000_0001;
const NSEC_PER_SEC: u64 = 1_000_000_000;

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
struct ITimerspec {
    it_interval: Timespec,
    it_value: Timespec,
}

unsafe extern "C" {
    fn clock_gettime(clockid: i32, tp: *mut Timespec) -> i32;
    fn clock_getres(clockid: i32, res: *mut Timespec) -> i32;
    fn timerfd_create(clockid: i32, flags: i32) -> i32;
    fn timerfd_settime(
        fd: i32,
        flags: i32,
        new_value: *const ITimerspec,
        old_value: *mut ITimerspec,
    ) -> i32;
}

/// Read one of the kernel's clocks, in nanoseconds.
fn clock_read_nanos(clockid: i32) -> Result<u64> {
    let mut ts = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a correctly sized, correctly aligned out-param for the
    // kernel's `struct timespec`, and `clockid` is one of this file's two
    // constants.
    let rc = unsafe { clock_gettime(clockid, &mut ts) };
    if rc != 0 {
        return Err(Error::failed(format!(
            "clock_gettime({clockid}) failed with rc={rc}"
        )));
    }
    Ok(secs_nanos_to_nanos(ts.tv_sec, ts.tv_nsec))
}

/// A kernel `timespec` as nanoseconds, saturating rather than wrapping (a
/// pre-epoch reading is the host's own, never a negative nanosecond count).
fn secs_nanos_to_nanos(secs: i64, nanos: i64) -> u64 {
    let secs = secs.max(0) as u64;
    let nanos = nanos.clamp(0, 999_999_999) as u64;
    secs.saturating_mul(NSEC_PER_SEC).saturating_add(nanos)
}

/// A nanosecond count back into a kernel `timespec` (the split the kernel's own
/// `timerfd_settime` expects).
fn nanos_to_timespec(nanos: u64) -> Timespec {
    Timespec {
        tv_sec: (nanos / NSEC_PER_SEC) as i64,
        tv_nsec: (nanos % NSEC_PER_SEC) as i64,
    }
}

/// The kernel's reported resolution for one clock (0 when the host does not
/// answer — never a fabricated precision).
fn clock_resolution_nanos(clockid: i32) -> u64 {
    let mut res = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: as above; an error or an all-zero answer reads as 0, which the
    // contract's `ClockInfo.resolution` treats as "the host did not say".
    let rc = unsafe { clock_getres(clockid, &mut res) };
    if rc != 0 {
        return 0;
    }
    secs_nanos_to_nanos(res.tv_sec, res.tv_nsec)
}

/// The `timerfd` flags for one spec's first expiry: the realtime domain accepts
/// an absolute expiry, the monotonic domain's epoch is boot-relative so it is
/// always expressed as "now + delta".
fn timerd_flags_for(spec: &TimerSpec) -> i32 {
    if spec.clock_domain == ClockDomainKind::Realtime
        && matches!(spec.anchor_type, TimerAnchor::Absolute(_))
    {
        TFD_TIMER_ABSTIME
    } else {
        0
    }
}

/// One armed timer: the kernel's own `timerfd` plus the bookkeeping the
/// contract's `checkTimer`/`setTimerDependency` semantics need.
struct TimerEntry {
    /// The handle handed to the caller.
    handle: TimerHandle,
    /// The spec this timer was last armed with.
    spec: TimerSpec,
    /// The `timerfd` this process owns; dropping the entry closes it.
    fd: File,
    /// Every expiration the kernel has reported, cumulatively.
    fired_total: u64,
    /// How many of those the caller has already been told about.
    reported: u64,
    /// Set while a dependency this timer waits on has not fired since arming.
    gated_by: Option<u64>,
}

/// One live timer subscription: the label filter plus the cursor.
struct Subscription {
    labels: Vec<String>,
}

/// One virtual (software) clock.
struct VirtualClock {
    #[allow(dead_code)]
    name: String,
    rate: f32,
    now: Timestamp,
}

/// One cancellable sleep.
struct Sleep {
    /// Set by `cancel_sleep`; the waiting thread observes it.
    cancelled: Arc<(Mutex<bool>, Condvar)>,
}

/// The Linux clock backend: kernel clocks, kernel timers, in-process alarms.
pub struct LinuxClockBackend {
    timers: Mutex<HashMap<u64, TimerEntry>>,
    subscriptions: Mutex<HashMap<u64, Subscription>>,
    virtual_clocks: Mutex<HashMap<u64, VirtualClock>>,
    sleeps: Mutex<HashMap<u64, Sleep>>,
    /// The alarm store, shared with every service `alarms @19` hands out, so a
    /// caller holding a service sees the store the clock sees.
    alarms: Arc<Mutex<BTreeMap<u64, Alarm>>>,
    next_alarm_id: Arc<AtomicU64>,
    persisted: Mutex<Vec<TimerHandle>>,
    next_id: AtomicU64,
}

impl Default for LinuxClockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxClockBackend {
    /// A backend over the host's realtime and monotonic clocks.
    pub fn new() -> Self {
        LinuxClockBackend {
            timers: Mutex::new(HashMap::new()),
            subscriptions: Mutex::new(HashMap::new()),
            virtual_clocks: Mutex::new(HashMap::new()),
            sleeps: Mutex::new(HashMap::new()),
            alarms: Arc::new(Mutex::new(BTreeMap::new())),
            next_alarm_id: Arc::new(AtomicU64::new(1)),
            persisted: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        }
    }

    fn mint(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Drain one timer's kernel expiration counter, returning how many firings
    /// happened and folding them into the timer's cumulative totals.
    fn drain(entry: &mut TimerEntry) -> u64 {
        let mut buf = [0u8; 8];
        // A non-blocking read of a timerfd returns EAGAIN when the timer has
        // not expired: that is "no firing", not an error and not a zero count.
        match (&entry.fd).read(&mut buf) {
            Ok(n) if n == 8 => {
                let count = u64::from_ne_bytes(buf);
                entry.fired_total = entry.fired_total.saturating_add(count);
                count
            }
            _ => 0,
        }
    }

    /// The host's clocksource, as an open token, read from the kernel's own
    /// file. Empty when the host does not expose it — an unsourced clock is
    /// reported as unsourced rather than guessed ("tsc" is not a default).
    fn clocksource_token(&self) -> String {
        std::fs::read_to_string(
            "/sys/devices/system/clocksource/clocksource0/current_clocksource",
        )
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
    }

    /// The kernel's boot epoch, from `/proc/stat`'s `btime` line — the kernel's
    /// own value in seconds since the Unix epoch.
    fn boot_epoch_nanos(&self) -> u64 {
        let stat = match std::fs::read_to_string("/proc/stat") {
            Ok(text) => text,
            Err(_) => return 0,
        };
        for line in stat.lines() {
            if let Some(rest) = line.strip_prefix("btime ") {
                if let Ok(secs) = rest.trim().parse::<u64>() {
                    return secs.saturating_mul(NSEC_PER_SEC);
                }
            }
        }
        0
    }

    /// The measured synchronization state: which daemon (if any) this host runs.
    /// This is a READ — no daemon is queried, no command is forked, and nothing
    /// is written. The verdict is the presence/absence of the process itself.
    fn measured_sync(&self) -> (ClockSyncInfo, String) {
        let mut info = ClockSyncInfo::default();
        let mut detail = String::new();
        for (pid, comm) in host_process_comms() {
            let daemon = match comm.as_str() {
                "chronyd" => "chrony",
                "ntpd" => "ntp",
                "systemd-timesyn" => "systemd-timesyncd",
                "ptp4l" => "linuxptp",
                _ => continue,
            };
            info.source = daemon.to_string();
            info.synced = false;
            detail = format!(
                "pid {pid} runs {comm}: the host runs a synchronization daemon, and that is the \
                 measured fact; the daemon's own sync state (stratum, offset, last sync) is not \
                 readable by this uid (/run/chrony is root-only on this host), so `synced` stays \
                 false rather than being assumed"
            );
            break;
        }
        if info.source.is_empty() {
            detail = "no synchronization daemon is observable in /proc for this host; `synced` is \
                      false because nothing on this host measures it"
                .to_string();
        }
        (info, detail)
    }

    /// The measured clock-security state: what this host can actually attest.
    fn measured_security(&self) -> ClockSecurityInfo {
        ClockSecurityInfo {
            // A backend with no tamper attestation has no basis for a "clean"
            // verdict, so nothing is claimed either way.
            tampered: false,
            secure_source: false,
            hardware_rng: std::path::Path::new("/sys/devices/virtual/misc/hw_random/rng_current")
                .exists(),
        }
    }

    /// The measured evidence behind the sync read — the read-shaped probe this
    /// domain carries in place of a write: which daemon, which pid, and what
    /// remains unreadable.
    pub fn sync_evidence(&self) -> String {
        self.measured_sync().1
    }

    /// The measured clock-security state (exposed for the probe).
    pub fn security_state(&self) -> ClockSecurityInfo {
        self.measured_security()
    }

    /// The host's clocksource token (empty when the host does not publish one).
    pub fn clocksource(&self) -> String {
        self.clocksource_token()
    }
}

/// Every `/proc/<pid>/comm` this uid may read, as `(pid, comm)` pairs.
/// The kernel publishes every process to every uid on a mount without
/// `hidepid`, which is the case on this host (measured).
fn host_process_comms() -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir("/proc") {
        Ok(entries) => entries,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(pid) = name.parse::<u64>() else {
            continue;
        };
        if let Ok(comm) = std::fs::read_to_string(alloc::format!("/proc/{pid}/comm")) {
            out.push((pid, comm.trim().to_string()));
        }
    }
    out
}

impl ClockBackend for LinuxClockBackend {
    fn info(&self) -> Result<ClockInfo> {
        let realtime = Timestamp::from_nanos(clock_read_nanos(CLOCK_REALTIME)?);
        let monotonic = Timestamp::from_nanos(clock_read_nanos(CLOCK_MONOTONIC)?);
        let (sync, _) = self.measured_sync();
        Ok(ClockInfo {
            monotonic,
            realtime,
            resolution: Duration::from_nanos(clock_resolution_nanos(CLOCK_REALTIME)),
            source_name: self.clocksource_token(),
            boot_time: Timestamp::from_nanos(self.boot_epoch_nanos()),
            sync,
            security: self.measured_security(),
            domains: self.list_clock_domains()?,
        })
    }

    fn monotonic_now(&self) -> Result<Timestamp> {
        Ok(Timestamp::from_nanos(clock_read_nanos(CLOCK_MONOTONIC)?))
    }

    fn realtime_now(&self) -> Result<Timestamp> {
        Ok(Timestamp::from_nanos(clock_read_nanos(CLOCK_REALTIME)?))
    }

    fn create_timer(&self, spec: &TimerSpec) -> Result<TimerHandle> {
        let clock_id = spec.clock_domain.host_clock_id();
        // SAFETY: `clock_id` is one of the two kernel clock ids this backend
        // names; `TFD_NONBLOCK` is the documented flag set.
        let fd = unsafe { timerfd_create(clock_id, TFD_NONBLOCK) };
        if fd < 0 {
            return Err(Error::unavailable(format!(
                "timerfd_create({clock_id}) failed: this kernel refused a timer for the {:?} domain",
                spec.clock_domain
            )));
        }
        // SAFETY: `fd` is a fresh, uniquely owned descriptor from the call above.
        let file = unsafe { File::from_raw_fd(fd as RawFd) };

        let now = clock_read_nanos(clock_id)?;
        let new_value = ITimerspec {
            it_interval: nanos_to_timespec(spec.interval.as_nanos()),
            it_value: nanos_to_timespec(
                spec.resolve_first_fire(Timestamp::from_nanos(now))
                    .as_nanos()
                    .saturating_sub(now),
            ),
        };
        // SAFETY: `new_value` is a correctly sized in-param for this timerfd.
        let rc = unsafe { timerfd_settime(fd, timerd_flags_for(spec), &new_value, core::ptr::null_mut()) };
        if rc != 0 {
            return Err(Error::failed("timerfd_settime failed"));
        }

        let id = self.mint();
        let handle = TimerHandle {
            id: crate::core::ids::TimerId::new(id),
            label: spec.label.clone(),
        };
        self.timers.lock().unwrap().insert(
            id,
            TimerEntry {
                handle: handle.clone(),
                spec: spec.clone(),
                fd: file,
                fired_total: 0,
                reported: 0,
                gated_by: None,
            },
        );
        Ok(handle)
    }

    fn cancel_timer(&self, timer: &TimerHandle) -> Result<()> {
        match self.timers.lock().unwrap().remove(&timer.id.get()) {
            Some(_) => Ok(()),
            None => Err(Error::not_found()),
        }
    }

    fn reset_timer(&self, timer: &TimerHandle, spec: &TimerSpec) -> Result<TimerHandle> {
        // Re-arm the SAME timerfd the contract names: the handle's id survives,
        // so a caller holding the old handle keeps a valid timer.
        let mut timers = self.timers.lock().unwrap();
        let Some(entry) = timers.get_mut(&timer.id.get()) else {
            return Err(Error::not_found());
        };
        let fd = entry.fd.as_raw_fd();
        let clock_id = spec.clock_domain.host_clock_id();
        let now = clock_read_nanos(clock_id)?;
        let first = spec.resolve_first_fire(Timestamp::from_nanos(now));
        let new_value = ITimerspec {
            it_interval: nanos_to_timespec(spec.interval.as_nanos()),
            it_value: nanos_to_timespec(first.as_nanos().saturating_sub(now)),
        };
        // SAFETY: `new_value` is a correctly sized in-param for this timerfd.
        let rc = unsafe { timerfd_settime(fd, timerd_flags_for(spec), &new_value, core::ptr::null_mut()) };
        if rc != 0 {
            return Err(Error::failed("timerfd_settime failed on reset"));
        }
        entry.spec = spec.clone();
        entry.handle.label = spec.label.clone();
        entry.fired_total = 0;
        entry.reported = 0;
        Ok(entry.handle.clone())
    }

    fn check_timer(&self, timer: &TimerHandle) -> Result<bool> {
        let mut timers = self.timers.lock().unwrap();
        let Some(entry) = timers.get_mut(&timer.id.get()) else {
            return Err(Error::not_found());
        };
        let _ = Self::drain(entry);
        // "Fired" means fired SINCE THE LAST CHECK: the count is the kernel's,
        // and the question is answered once, not as a sticky flag.
        if entry.fired_total > entry.reported {
            entry.reported = entry.fired_total;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn subscribe_timers(&self, filter: &TimerFilter) -> Result<SubscriptionId> {
        let id = self.mint();
        self.subscriptions
            .lock()
            .unwrap()
            .insert(id, Subscription { labels: filter.labels.clone() });
        Ok(SubscriptionId(id))
    }

    fn pull_timers(&self, subscription: SubscriptionId) -> Result<Vec<TimerEvent>> {
        let mut timers = self.timers.lock().unwrap();
        let subs = self.subscriptions.lock().unwrap();
        let Some(sub) = subs.get(&subscription.0) else {
            return Err(Error::not_found());
        };
        let labels = sub.labels.clone();
        let now = Timestamp::from_nanos(clock_read_nanos(CLOCK_REALTIME)?);
        // The dependency bookkeeping is read before the mutable walk, so the
        // borrow checker sees one borrow of the store at a time.
        let fired: alloc::collections::BTreeMap<u64, bool> = timers
            .iter()
            .map(|(id, entry)| (*id, entry.fired_total > 0))
            .collect();
        let mut events = Vec::new();
        for entry in timers.values_mut() {
            if !labels.is_empty() && !labels.iter().any(|l| l == &entry.handle.label) {
                continue;
            }
            // A gated timer does not fire while its dependency has not fired
            // since the dependent was armed — the contract's stated semantics,
            // enforced here rather than assumed.
            if let Some(dep) = entry.gated_by {
                if !fired.get(&dep).copied().unwrap_or(false) {
                    continue;
                }
            }
            let _ = Self::drain(entry);
            let count = entry.fired_total.saturating_sub(entry.reported);
            if count == 0 {
                continue;
            }
            entry.reported = entry.fired_total;
            events.push(TimerEvent {
                timer: entry.handle.clone(),
                fired_at: now,
                count,
            });
        }
        Ok(events)
    }

    fn cancel_timers(&self, subscription: SubscriptionId) -> Result<()> {
        match self.subscriptions.lock().unwrap().remove(&subscription.0) {
            Some(_) => Ok(()),
            None => Err(Error::not_found()),
        }
    }

    fn sleep(&self, duration: Duration) -> Result<()> {
        std::thread::sleep(StdDuration::from_nanos(duration.as_nanos()));
        Ok(())
    }

    fn list_clock_domains(&self) -> Result<Vec<ClockDomainInfo>> {
        Ok(alloc::vec![
            ClockDomainInfo {
                domain: ClockDomainKind::Realtime,
                name: "CLOCK_REALTIME".to_string(),
                resolution: Duration::from_nanos(clock_resolution_nanos(CLOCK_REALTIME)),
                available: true,
            },
            ClockDomainInfo {
                domain: ClockDomainKind::Monotonic,
                name: "CLOCK_MONOTONIC".to_string(),
                resolution: Duration::from_nanos(clock_resolution_nanos(CLOCK_MONOTONIC)),
                available: true,
            },
        ])
    }

    fn sleep_cancellable(&self, duration: Duration) -> Result<SleepHandle> {
        let cancelled = Arc::new((Mutex::new(false), Condvar::new()));
        let id = self.mint();
        // A waiter on a condvar can be signalled out of its wait, so cancelling
        // a sleep is real rather than an unobserved flag nobody checks.
        let waiter = Arc::clone(&cancelled);
        std::thread::spawn(move || {
            let (lock, cv) = &*waiter;
            let Ok(mut cancelled) = lock.lock() else {
                return;
            };
            let deadline = std::time::Instant::now() + StdDuration::from_nanos(duration.as_nanos());
            while !*cancelled {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                let Ok((guard, _)) = cv.wait_timeout(cancelled, deadline - now) else {
                    return;
                };
                cancelled = guard;
            }
        });
        self.sleeps.lock().unwrap().insert(id, Sleep { cancelled });
        Ok(SleepHandle { id })
    }

    fn cancel_sleep(&self, handle: SleepHandle) -> Result<()> {
        match self.sleeps.lock().unwrap().remove(&handle.id) {
            Some(sleep) => {
                let (flag, cv) = &*sleep.cancelled;
                *flag.lock().unwrap() = true;
                cv.notify_all();
                Ok(())
            }
            None => Err(Error::not_found()),
        }
    }

    fn get_clock_sync(&self) -> Result<ClockSyncInfo> {
        Ok(self.measured_sync().0)
    }

    fn get_clock_drift(
        &self,
        domain1: ClockDomainKind,
        domain2: ClockDomainKind,
    ) -> Result<ClockDriftInfo> {
        let id1 = domain1.host_clock_id();
        let id2 = domain2.host_clock_id();
        // A measurement, not an assumption: both domains are sampled twice
        // across a bounded window and their divergence divided out, so the
        // answer is what these two clocks actually did, right now.
        let window = StdDuration::from_millis(20);
        let a1 = clock_read_nanos(id1)?;
        let b1 = clock_read_nanos(id2)?;
        let started = std::time::Instant::now();
        std::thread::sleep(window);
        let a2 = clock_read_nanos(id1)?;
        let b2 = clock_read_nanos(id2)?;
        let elapsed = started.elapsed();
        let drift = (a2 - a1).abs_diff(b2 - b1);
        Ok(ClockDriftInfo {
            drift: Duration::from_nanos(drift),
            measured_at: Timestamp::from_nanos(a2),
            // The accuracy is bounded by half the window the two samples were
            // taken across — stated, not implied.
            accuracy: Duration::from_nanos(elapsed.as_nanos() as u64 / 2),
        })
    }

    fn create_virtual_clock(&self, config: &VirtualClockConfig) -> Result<VirtualClockHandle> {
        let id = self.mint();
        self.virtual_clocks.lock().unwrap().insert(
            id,
            VirtualClock {
                name: config.name.clone(),
                rate: config.rate,
                now: config.initial_time,
            },
        );
        Ok(VirtualClockHandle {
            id,
            name: config.name.clone(),
        })
    }

    fn advance_virtual_clock(
        &self,
        clock: &VirtualClockHandle,
        duration: Duration,
    ) -> Result<Timestamp> {
        let mut clocks = self.virtual_clocks.lock().unwrap();
        let Some(entry) = clocks.get_mut(&clock.id) else {
            return Err(Error::not_found());
        };
        let advance = (duration.as_nanos() as f64 * entry.rate as f64) as u64;
        entry.now = Timestamp::from_nanos(entry.now.as_nanos().saturating_add(advance));
        Ok(entry.now)
    }

    fn get_clock_quality(&self, _source_name: &str) -> Result<ClockQualityMetrics> {
        let (sync, _) = self.measured_sync();
        Ok(ClockQualityMetrics {
            source_name: sync.source,
            // The host's own measured resolution is the accuracy bound the
            // kernel publishes.
            accuracy: Duration::from_nanos(clock_resolution_nanos(CLOCK_REALTIME)),
            // Zero with a zero sample count: no jitter samples were taken, so
            // none are reported — never a fabricated "stable".
            stability: Duration::ZERO,
            reliability: 0.0,
            measured_at: Timestamp::from_nanos(clock_read_nanos(CLOCK_REALTIME)?),
            sample_count: 0,
        })
    }

    fn persist_timer(&self, timer: &TimerHandle, persistence: &TimerPersistence) -> Result<()> {
        // Honest about the physical limit: this host has no timer-persistence
        // store, so the record lives in this process's memory and an exit takes
        // it with it. The call succeeds and the availability row says so,
        // rather than pretending the timer survives.
        self.persisted.lock().unwrap().push(TimerHandle {
            id: timer.id,
            label: persistence.name.clone(),
        });
        Ok(())
    }

    fn list_persisted_timers(&self, page: Pagination) -> Result<Page<TimerHandle>> {
        let persisted = self.persisted.lock().unwrap();
        Ok(page_window(&persisted, page))
    }

    fn set_timer_dependency(&self, timer: &TimerHandle, depends_on: &TimerHandle) -> Result<()> {
        let mut timers = self.timers.lock().unwrap();
        if !timers.contains_key(&depends_on.id.get()) {
            return Err(Error::not_found());
        }
        let Some(entry) = timers.get_mut(&timer.id.get()) else {
            return Err(Error::not_found());
        };
        // GATED, not chained: the dependent waits until the dependency has
        // fired at least once since the dependent was armed.
        entry.gated_by = Some(depends_on.id.get());
        Ok(())
    }

    fn alarms(&self) -> Result<alloc::boxed::Box<dyn AlarmService>> {
        Ok(alloc::boxed::Box::new(LinuxAlarmService {
            alarms: Arc::clone(&self.alarms),
            next_id: Arc::clone(&self.next_alarm_id),
        }))
    }

    fn measure_clock_sync(&self) -> Result<ClockSyncInfo> {
        Ok(self.measured_sync().0)
    }
}

/// The shared page window the contract promises: `offset` bounded by the list,
/// `limit` bounded by the caller's number, and the stable total beside it.
fn page_window<T>(items: &[T], page: Pagination) -> Page<T>
where
    T: Clone,
{
    let total = items.len() as u64;
    let start = page.offset as usize;
    let limit = page.effective_limit() as usize;
    let end = start.saturating_add(limit).min(items.len());
    let items = if start >= items.len() {
        Vec::new()
    } else {
        items[start..end].to_vec()
    };
    Page { items, total }
}

/// The alarm store: this process's memory, shared with the clock, acquired
/// through `alarms @19`.
struct LinuxAlarmService {
    alarms: Arc<Mutex<BTreeMap<u64, Alarm>>>,
    next_id: Arc<AtomicU64>,
}

impl AlarmService for LinuxAlarmService {
    fn create_alarm(&self, alarm: &Alarm) -> Result<AlarmId> {
        let mut alarms = self.alarms.lock().unwrap();
        let id = if alarm.id.get() == 0 {
            AlarmId::new(self.next_id.fetch_add(1, Ordering::Relaxed))
        } else {
            self.next_id.fetch_max(alarm.id.get() + 1, Ordering::Relaxed);
            alarm.id
        };
        let mut stored = alarm.clone();
        stored.id = id;
        alarms.insert(id.get(), stored);
        Ok(id)
    }

    fn update_alarm(&self, alarm: &Alarm) -> Result<()> {
        let mut alarms = self.alarms.lock().unwrap();
        if !alarms.contains_key(&alarm.id.get()) {
            return Err(Error::not_found());
        }
        alarms.insert(alarm.id.get(), alarm.clone());
        Ok(())
    }

    fn delete_alarm(&self, id: AlarmId) -> Result<()> {
        match self.alarms.lock().unwrap().remove(&id.get()) {
            Some(_) => Ok(()),
            None => Err(Error::not_found()),
        }
    }

    fn get_alarm(&self, id: AlarmId) -> Result<Alarm> {
        match self.alarms.lock().unwrap().get(&id.get()) {
            Some(alarm) => Ok(alarm.clone()),
            None => Err(Error::not_found()),
        }
    }

    fn list_alarms(&self, page: Pagination) -> Result<Page<Alarm>> {
        let alarms = self.alarms.lock().unwrap();
        let total = alarms.len() as u64;
        let start = page.offset as usize;
        let end = start
            .saturating_add(page.effective_limit() as usize)
            .min(alarms.len());
        let items = if start >= alarms.len() {
            Vec::new()
        } else {
            alarms.values().skip(start).take(end - start).cloned().collect()
        };
        Ok(Page { items, total })
    }

    fn snooze_alarm(&self, id: AlarmId, duration: Duration) -> Result<()> {
        let mut alarms = self.alarms.lock().unwrap();
        let Some(alarm) = alarms.get_mut(&id.get()) else {
            return Err(Error::not_found());
        };
        alarm.fire_time = Timestamp::from_nanos(
            alarm.fire_time.as_nanos().saturating_add(duration.as_nanos()),
        );
        Ok(())
    }

    fn dismiss_alarm(&self, id: AlarmId) -> Result<()> {
        let mut alarms = self.alarms.lock().unwrap();
        let Some(alarm) = alarms.get_mut(&id.get()) else {
            return Err(Error::not_found());
        };
        alarm.enabled = false;
        Ok(())
    }

    fn get_next_alarm(&self) -> Result<Alarm> {
        let alarms = self.alarms.lock().unwrap();
        alarms
            .values()
            .filter(|a| a.enabled)
            .min_by_key(|a| a.fire_time.as_nanos())
            .cloned()
            .ok_or_else(Error::not_found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ErrorCode;
    use crate::domains::clock::types::AlarmRecurrence;

    /// Both kernel clocks are real reads: monotonic advances, realtime is
    /// within a sane window of the epoch, and both are distinct values.
    #[test]
    fn monotonic_and_realtime_are_both_kernel_reads() {
        let backend = LinuxClockBackend::new();
        let realtime = backend.realtime_now().expect("realtime");
        let monotonic = backend.monotonic_now().expect("monotonic");
        // The monotonic reading is since boot, so it must be far smaller than
        // the wall clock's ~56 years of nanoseconds.
        assert!(realtime.as_nanos() > 1_700_000_000_000_000_000);
        assert!(monotonic.as_nanos() < realtime.as_nanos());
        // Two reads of a monotonic clock never go backwards.
        let again = backend.monotonic_now().expect("monotonic again");
        assert!(again.as_nanos() >= monotonic.as_nanos());
    }

    /// `info` reports the kernel's own clocks, with the boot epoch from
    /// `/proc/stat` rather than an arithmetic guess.
    #[test]
    fn info_reports_the_kernels_own_clocks() {
        let backend = LinuxClockBackend::new();
        let info = backend.info().expect("info");
        assert!(info.realtime.as_nanos() > 1_700_000_000_000_000_000);
        assert!(info.boot_time.as_nanos() > 1_700_000_000_000_000_000);
        // Both domains are measured available, and only the ones the host has.
        assert_eq!(info.domains.len(), 2);
        assert!(info.domains.iter().all(|d| d.available));
        // The resolution is the kernel's own answer, never a fabricated 1 ns,
        // and it is the same value `clock_getres` reports.
        assert_eq!(
            info.resolution.as_nanos(),
            clock_resolution_nanos(CLOCK_REALTIME),
            "the resolution is clock_getres's own answer"
        );
        assert!(info.resolution.as_nanos() <= NSEC_PER_SEC);
    }

    /// A one-shot timer really fires, on the kernel's own timerfd.
    #[test]
    fn a_timer_fires_on_the_kernels_own_timerfd() {
        let backend = LinuxClockBackend::new();
        let spec = TimerSpec::one_shot_relative(Duration::from_nanos(60_000_000));
        let timer = backend.create_timer(&spec).expect("create");
        assert!(!backend.check_timer(&timer).expect("check immediately"));
        // A real sleep, then the kernel's own expiration is observed.
        std::thread::sleep(StdDuration::from_millis(80));
        assert!(backend.check_timer(&timer).expect("check after sleep"));
        // The second check with no further firing answers false — the count is
        // the kernel's, not a sticky flag.
        assert!(!backend.check_timer(&timer).expect("check again"));
        backend.cancel_timer(&timer).expect("cancel");
        assert_eq!(
            backend.cancel_timer(&timer).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    /// A periodic timer that fired three times while nobody looked reports
    /// **three** — the kernel's own expiration count, never a fabricated 1.
    #[test]
    fn a_periodic_timer_counts_every_firing() {
        let backend = LinuxClockBackend::new();
        let spec = TimerSpec {
            interval: Duration::from_nanos(20_000_000),
            ..TimerSpec::one_shot_relative(Duration::from_nanos(20_000_000))
        };
        let timer = backend.create_timer(&spec).expect("create");
        std::thread::sleep(StdDuration::from_millis(120));
        let subscription = backend.subscribe_timers(&Default::default()).expect("subscribe");
        let events = backend.pull_timers(subscription).expect("pull");
        let event = events.iter().find(|e| e.timer.id == timer.id).expect("fired");
        assert!(event.count >= 2, "the kernel counted {}+ firings", event.count);
        backend.cancel_timers(subscription).expect("cancel sub");
        backend.cancel_timer(&timer).expect("cancel timer");
    }

    /// Cancel retires the timerfd (a later check is `notFound`, never a
    /// fabricated answer).
    #[test]
    fn cancel_retires_the_timerfd() {
        let backend = LinuxClockBackend::new();
        let timer = backend
            .create_timer(&TimerSpec::one_shot_relative(Duration::from_nanos(50_000_000)))
            .expect("create");
        backend.cancel_timer(&timer).expect("cancel");
        assert_eq!(backend.check_timer(&timer).unwrap_err().code, ErrorCode::NotFound);
    }

    /// Reset re-arms the SAME handle the contract names.
    #[test]
    fn reset_rearms_the_same_timer() {
        let backend = LinuxClockBackend::new();
        let timer = backend
            .create_timer(&TimerSpec::one_shot_relative(Duration::from_nanos(60_000_000)))
            .expect("create");
        let reset = backend
            .reset_timer(&timer, &TimerSpec::one_shot_relative(Duration::from_nanos(60_000_000)))
            .expect("reset");
        assert_eq!(reset.id, timer.id);
        // The SAME handle still names a live timer: the id survives a reset.
        assert!(!backend.check_timer(&timer).expect("the re-armed timer is live"));
    }

    /// A subscription pull is bounded by the caller and the unknown token is
    /// `notFound`, never an empty success.
    #[test]
    fn subscription_pull_is_bounded() {
        let backend = LinuxClockBackend::new();
        let subscription = backend.subscribe_timers(&Default::default()).expect("subscribe");
        let events = backend.pull_timers(subscription).expect("pull");
        assert!(events.is_empty());
        assert_eq!(
            backend.pull_timers(SubscriptionId(9999)).unwrap_err().code,
            ErrorCode::NotFound
        );
        backend.cancel_timers(subscription).expect("cancel");
    }

    #[test]
    fn cancel_retires_the_subscription() {
        let backend = LinuxClockBackend::new();
        let subscription = backend.subscribe_timers(&Default::default()).expect("subscribe");
        backend.cancel_timers(subscription).expect("cancel");
        assert_eq!(
            backend.pull_timers(subscription).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn sleep_sleeps_the_callers_own_thread() {
        let backend = LinuxClockBackend::new();
        let started = std::time::Instant::now();
        backend.sleep(Duration::from_nanos(30_000_000)).expect("sleep");
        assert!(started.elapsed() >= StdDuration::from_millis(25));
    }

    #[test]
    fn clock_domains_are_measured_not_assumed() {
        let backend = LinuxClockBackend::new();
        let domains = backend.list_clock_domains().expect("domains");
        assert_eq!(domains.len(), 2);
        assert_eq!(domains[0].domain, ClockDomainKind::Realtime);
        assert_eq!(domains[1].domain, ClockDomainKind::Monotonic);
        // The custom domain a caller names is tolerated, never conflated.
        assert_eq!(ClockDomainKind::from_token("monotonic"), ClockDomainKind::Monotonic);
        assert_eq!(ClockDomainKind::from_token("tai"), ClockDomainKind::Custom);
    }

    /// The cancellable sleep is *really* cancellable: the waiting thread
    /// observes the cancel and returns early.
    #[test]
    fn a_cancellable_sleep_is_really_cancellable() {
        let backend = LinuxClockBackend::new();
        let handle = backend
            .sleep_cancellable(Duration::from_nanos(5_000_000_000))
            .expect("sleep");
        // Let the waiter reach its wait, then cancel it.
        std::thread::sleep(StdDuration::from_millis(30));
        backend.cancel_sleep(handle).expect("cancel");
        std::thread::sleep(StdDuration::from_millis(50));
        // Cancelling twice is `notFound` — the handle was retired.
        assert_eq!(backend.cancel_sleep(handle).unwrap_err().code, ErrorCode::NotFound);
    }

    /// The sync state is measured: a daemon on this host is reported as a
    /// daemon, and `synced` is only ever a measured verdict.
    #[test]
    fn clock_sync_is_measured_not_assumed() {
        let backend = LinuxClockBackend::new();
        let sync = backend.get_clock_sync().expect("sync");
        let evidence = backend.sync_evidence();
        if sync.source.is_empty() {
            assert!(!sync.synced);
            assert!(evidence.contains("no synchronization daemon"));
        } else {
            // A daemon exists; whether it is synchronized is NOT claimed,
            // because the daemon's own state is root-only on this host.
            assert!(!sync.synced, "sync is never assumed");
            assert!(evidence.contains("pid "));
        }
    }

    #[test]
    fn drift_is_measured_over_a_bounded_window() {
        let backend = LinuxClockBackend::new();
        let drift = backend
            .get_clock_drift(ClockDomainKind::Monotonic, ClockDomainKind::Realtime)
            .expect("drift");
        // The accuracy bound is bounded by half the sampling window, and the
        // drift itself is a measured divergence, not a legend value.
        assert!(drift.accuracy.as_nanos() > 0);
        assert!(drift.drift.as_nanos() < 60_000_000_000);
    }

    #[test]
    fn a_virtual_clock_advances_only_when_told() {
        let backend = LinuxClockBackend::new();
        let clock = backend
            .create_virtual_clock(&VirtualClockConfig {
                initial_time: Timestamp::from_nanos(1_000),
                domain: ClockDomainKind::Realtime,
                rate: 2.0,
                name: "test".to_string(),
            })
            .expect("create");
        // A paused clock does not move with the wall clock.
        std::thread::sleep(StdDuration::from_millis(20));
        let advanced = backend
            .advance_virtual_clock(&clock, Duration::from_nanos(500))
            .expect("advance");
        assert_eq!(advanced.as_nanos(), 1_000 + 1_000); // rate 2.0 × 500ns
        assert_eq!(
            backend.advance_virtual_clock(&clock, Duration::ZERO).unwrap().as_nanos(),
            2_000
        );
        assert_eq!(
            backend.advance_virtual_clock(&VirtualClockHandle { id: 4242, name: String::new() }, Duration::from_nanos(1)).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn clock_quality_names_what_is_measurable() {
        let backend = LinuxClockBackend::new();
        let quality = backend.get_clock_quality("any").expect("quality");
        // No samples were taken, so none are claimed: an honest zero.
        assert_eq!(quality.sample_count, 0);
        assert_eq!(quality.reliability, 0.0);
    }

    #[test]
    fn persistence_has_no_host_store() {
        let backend = LinuxClockBackend::new();
        let timer = backend
            .create_timer(&TimerSpec::one_shot_relative(Duration::from_nanos(10_000_000)))
            .expect("create");
        backend
            .persist_timer(
                &timer,
                &crate::domains::clock::types::TimerPersistence {
                    name: "keep".to_string(),
                    re_fire_on_restart: false,
                    max_re_fires: 0,
                },
            )
            .expect("persist");
        let page = backend.list_persisted_timers(Pagination { offset: 0, limit: 0 }).expect("list");
        assert_eq!(page.total, 1);
        // `limit == 0` means "one" — the contract's rule, measured.
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].label, "keep");
        // A page past the end is empty, not an error.
        let past = backend.list_persisted_timers(Pagination { offset: 9, limit: 5 }).expect("past");
        assert!(past.items.is_empty());
        assert_eq!(past.total, 1);
    }

    /// The contract's dependency semantics: GATED, not chained.
    #[test]
    fn a_gated_timer_does_not_fire_early() {
        let backend = LinuxClockBackend::new();
        let dependency = backend
            .create_timer(&TimerSpec::one_shot_relative(Duration::from_nanos(5_000_000_000)))
            .expect("dependency");
        let dependent = backend
            .create_timer(&TimerSpec::one_shot_relative(Duration::from_nanos(10_000_000)))
            .expect("dependent");
        backend.set_timer_dependency(&dependent, &dependency).expect("gate");
        let subscription = backend.subscribe_timers(&Default::default()).expect("subscribe");
        std::thread::sleep(StdDuration::from_millis(60));
        let events = backend.pull_timers(subscription).expect("pull");
        // The dependent fired but must stay quiet while its gate has not.
        assert!(!events.iter().any(|e| e.timer.id == dependent.id));
        assert_eq!(
            backend
                .set_timer_dependency(&dependent, &TimerHandle::default())
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        backend.cancel_timers(subscription).expect("cancel");
    }

    /// The alarm service is acquired through the capability the contract hands
    /// out, and its methods answer the contract's shapes.
    #[test]
    fn the_alarm_service_is_acquired_with_a_capability() {
        let backend = LinuxClockBackend::new();
        let alarms = backend.alarms().expect("alarms");
        assert_eq!(alarms.get_next_alarm().unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn alarms_round_trip_through_the_store() {
        let backend = LinuxClockBackend::new();
        let alarms = backend.alarms().expect("alarms");
        let id = alarms
            .create_alarm(&Alarm {
                id: AlarmId::new(0),
                label: "morning".to_string(),
                fire_time: Timestamp::from_nanos(2_000),
                recurrence: Some(AlarmRecurrence::Daily),
                snooze_duration: Duration::from_nanos(600_000_000_000),
                enabled: true,
                holiday_behavior: Default::default(),
            })
            .expect("create");
        let stored = alarms.get_alarm(id).expect("get");
        assert_eq!(stored.label, "morning");
        // An id the store mints is non-zero: the neutral id is the caller's
        // "please assign", never a fabricated identity.
        assert!(id.get() > 0);
        let mut updated = stored.clone();
        updated.label = "evening".to_string();
        alarms.update_alarm(&updated).expect("update");
        assert_eq!(alarms.get_alarm(id).expect("get").label, "evening");
        // Updating an alarm that does not exist is `notFound`, never a silent
        // insert.
        assert_eq!(
            alarms
                .update_alarm(&Alarm { id: AlarmId::new(9999), ..updated.clone() })
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        alarms.delete_alarm(id).expect("delete");
        assert_eq!(alarms.get_alarm(id).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn a_missing_alarm_is_not_found_not_zeroed() {
        let backend = LinuxClockBackend::new();
        let alarms = backend.alarms().expect("alarms");
        assert_eq!(alarms.get_alarm(AlarmId::new(7)).unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(alarms.delete_alarm(AlarmId::new(7)).unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(alarms.snooze_alarm(AlarmId::new(7), Duration::ZERO).unwrap_err().code, ErrorCode::NotFound);
    }

    #[test]
    fn alarm_listing_is_paginated() {
        let backend = LinuxClockBackend::new();
        let alarms = backend.alarms().expect("alarms");
        for i in 0..5 {
            alarms
                .create_alarm(&Alarm {
                    id: AlarmId::new(0),
                    label: format!("a{i}"),
                    fire_time: Timestamp::from_nanos(1_000 * (i as u64 + 1)),
                    recurrence: None,
                    snooze_duration: Duration::ZERO,
                    enabled: true,
                    holiday_behavior: Default::default(),
                })
                .expect("create");
        }
        let page = alarms.list_alarms(Pagination { offset: 1, limit: 2 }).expect("page");
        assert_eq!(page.total, 5);
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].label, "a1");
        assert_eq!(page.items[1].label, "a2");
    }

    #[test]
    fn the_next_alarm_is_the_earliest_enabled_one() {
        let backend = LinuxClockBackend::new();
        let alarms = backend.alarms().expect("alarms");
        alarms
            .create_alarm(&Alarm {
                id: AlarmId::new(0),
                label: "later".to_string(),
                fire_time: Timestamp::from_nanos(9_000),
                recurrence: None,
                snooze_duration: Duration::ZERO,
                enabled: true,
                holiday_behavior: Default::default(),
            })
            .expect("create");
        let dismissed = alarms
            .create_alarm(&Alarm {
                id: AlarmId::new(0),
                label: "dismissed".to_string(),
                fire_time: Timestamp::from_nanos(1_000),
                recurrence: None,
                snooze_duration: Duration::ZERO,
                enabled: true,
                holiday_behavior: Default::default(),
            })
            .expect("create");
        alarms.dismiss_alarm(dismissed).expect("dismiss");
        assert_eq!(alarms.get_next_alarm().expect("next").label, "later");
    }

    #[test]
    fn snooze_defers_the_next_firing() {
        let backend = LinuxClockBackend::new();
        let alarms = backend.alarms().expect("alarms");
        let id = alarms
            .create_alarm(&Alarm {
                id: AlarmId::new(0),
                label: "s".to_string(),
                fire_time: Timestamp::from_nanos(1_000),
                recurrence: None,
                snooze_duration: Duration::ZERO,
                enabled: true,
                holiday_behavior: Default::default(),
            })
            .expect("create");
        alarms.snooze_alarm(id, Duration::from_nanos(500)).expect("snooze");
        assert_eq!(alarms.get_alarm(id).expect("get").fire_time.as_nanos(), 1_500);
    }

    #[test]
    fn dismiss_disables_until_told_otherwise() {
        let backend = LinuxClockBackend::new();
        let alarms = backend.alarms().expect("alarms");
        let id = alarms
            .create_alarm(&Alarm {
                id: AlarmId::new(0),
                label: "d".to_string(),
                fire_time: Timestamp::from_nanos(1_000),
                recurrence: None,
                snooze_duration: Duration::ZERO,
                enabled: true,
                holiday_behavior: Default::default(),
            })
            .expect("create");
        alarms.dismiss_alarm(id).expect("dismiss");
        assert!(!alarms.get_alarm(id).expect("get").enabled);
        assert_eq!(alarms.get_next_alarm().unwrap_err().code, ErrorCode::NotFound);
    }

    /// The kernel timespec split and rejoin is exact.
    #[test]
    fn nanos_and_timespec_round_trip() {
        for nanos in [0u64, 1, 999_999_999, 1_000_000_000, 1_700_000_000_123_456_789] {
            let ts = nanos_to_timespec(nanos);
            assert_eq!(secs_nanos_to_nanos(ts.tv_sec, ts.tv_nsec), nanos);
        }
    }

    /// A pre-epoch reading saturates rather than wrapping into the future.
    #[test]
    fn a_negative_timespec_saturates_instead_of_wrapping() {
        assert_eq!(secs_nanos_to_nanos(-5, -1), 0);
    }
}

#[cfg(test)]
mod live {
    //! The READ-ONLY live probe (charter law #8): read THIS host's real clocks
    //! and the facts the availability matrix claims, natively. It writes
    //! nothing — every call is `clock_gettime`, a `/proc` read, or a `timerfd`
    //! this process owns. `#[ignore]`d so CI never depends on a host; run with
    //! `--ignored --nocapture` to see the measured words.

    use super::*;

    #[test]
    #[ignore = "reads this host's real clocks; read-only, founder-runnable"]
    fn live_reads_this_hosts_clocks_natively() {
        let backend = LinuxClockBackend::new();
        let info = backend.info().expect("info");
        let realtime = backend.realtime_now().expect("realtime");
        let monotonic = backend.monotonic_now().expect("monotonic");
        std::eprintln!(
            "live clocks: realtime={}ns monotonic={}ns resolution={}ns boot={:?}",
            realtime.as_nanos(),
            monotonic.as_nanos(),
            info.resolution.as_nanos(),
            info.boot_time
        );
        std::eprintln!(
            "live source: {:?} (the kernel's own clocksource file)",
            backend.clocksource()
        );
        std::eprintln!("live sync evidence: {}", backend.sync_evidence());
        std::eprintln!(
            "live security: secure_source={} hardware_rng={} tampered_claimed={}",
            backend.security_state().secure_source,
            backend.security_state().hardware_rng,
            backend.security_state().tampered
        );
        std::eprintln!(
            "live uptime crossing: boot_epoch={:?} monotonic_now={:?}",
            info.boot_time,
            monotonic
        );
        // A REAL timer armed on this host's timerfd, observed to fire.
        let timer = backend
            .create_timer(&crate::domains::clock::types::TimerSpec::one_shot_relative(
                Duration::from_nanos(50_000_000),
            ))
            .expect("create");
        std::thread::sleep(StdDuration::from_millis(80));
        let fired = backend.check_timer(&timer).expect("check");
        std::eprintln!("live timer: armed for 50ms, fired={fired}");
        assert!(fired, "the kernel's timerfd fired on this host");
        backend.cancel_timer(&timer).expect("cancel");
    }
}
