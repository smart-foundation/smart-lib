//! The shell's SESSION: one backend set per process, the read counts a caller
//! can measure, and the clock-timer ledger that survives a process honestly.
//!
//! ## Why a session exists
//!
//! Every command used to rebuild its backend (`shell.rs` constructed a fresh
//! `Linux…Backend::new()` per line), so a timer armed by one verb was
//! invisible to the next: `clock timer create` printed `Timer #1 … a real
//! timerfd this process owns` and the very next line's `clock timer list`
//! answered `(0) … the timers this process armed` — the state died with the
//! command that made it. A REPL line and a `-c` line and a piped script line
//! now all drive the SAME backends, built once per process and memoized per
//! domain, so `create → list → check` composes and `watch` observes what the
//! session itself armed.
//!
//! ## What may survive the process — and what may not
//!
//! A `timerfd` is a kernel object owned by the process that armed it: no other
//! process can read its expiration counter, and it cannot be handed over. The
//! engine's own matrix agrees — `persistTimer` / `listPersistedTimers` are
//! DEGRADED with "this host carries no timer-persistence store", a physical
//! limit the engine names per row. So the shell does not fake one:
//!
//! * **within a session** the timers are live and checkable through the engine;
//! * **across invocations** what genuinely survives is the LEDGER FACT — a
//!   small local state file recording which pid armed which timer, when, with
//!   what spec — so a fresh `smart` can name the timers a live sibling session
//!   still owns and the ones whose owner (and fd) are gone. It never claims a
//!   firing: the kernel's counter died with the fd, and the row says so.
//!
//! Hermetic by construction: a `SessionState` built the library's way (tests,
//! `Shell::new`) has no state file and writes nothing, ever. Only the binary
//! path ([`ClockSession::persistent`]) resolves the state directory.

use std::path::PathBuf;

/// One per-domain read count, as the session measured it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadCount {
    pub domain: &'static str,
    /// How many times a command evaluated this domain's data (each is a real
    /// read against the session's backend).
    pub reads: u64,
    /// How many backends this domain built: one per session, zero after the
    /// first command. A non-zero build count on a later command is the rot
    /// this session replaced, measured.
    pub builds: u64,
}

/// One timer a session armed, as the ledger's own row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerRecord {
    /// The process that armed it (the owner of the kernel fd).
    pub pid: u32,
    /// The timer id that process's engine handed back.
    pub id: u64,
    pub label: String,
    /// The one-shot delay in milliseconds before the first firing.
    pub after_ms: u64,
    /// The periodic interval in milliseconds (0 is one-shot).
    pub interval_ms: u64,
    /// The clock the timer was armed on (realtime / monotonic).
    pub clock: String,
    /// When it was armed, in the engine's own epoch nanoseconds.
    pub armed_at_nanos: u128,
    /// The owning session, as `pid:armed_nanos` — the identity a later
    /// process uses to tell its own rows from a sibling's.
    pub session: String,
}

/// Where a ledger row stands when a fresh process reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerOwnership {
    /// This session armed it: it is live and checkable here.
    Live,
    /// Another process armed it and is still running: the kernel fd is that
    /// process's, so `timer check` belongs to that session.
    Owned,
    /// The owner exited: the timerfd died with it. Named, never resurrected.
    Gone,
}

/// A ledger row as `timer list` renders it: the record plus the measured
/// ownership, resolved against the host's own `/proc` (a read, law #8).
#[derive(Debug, Clone)]
pub struct TimerRow {
    pub record: TimerRecord,
    pub ownership: TimerOwnership,
}

/// The session's shared state: the per-domain read counts, the timer ledger
/// and the watch ledger. No state file unless [`SessionState::persistent`]
/// resolved one.
#[derive(Debug, Default)]
pub struct SessionState {
    counts: Vec<ReadCount>,
    pub clock: ClockSession,
    pub files_watch: WatchSession,
}

impl SessionState {
    /// The hermetic session: backends memoized per domain, NO state file, no
    /// writes anywhere. This is what the library's `Shell::new` builds and
    /// what every test drives.
    pub fn hermetic() -> Self {
        SessionState::default()
    }

    /// The persistent session: the clock and watch ledgers are shared with the
    /// next process through small state files (see the module docs).
    pub fn persistent() -> Self {
        SessionState {
            counts: Vec::new(),
            clock: ClockSession::persistent(),
            files_watch: WatchSession::persistent(),
        }
    }

    /// Count one domain read. `built` is 1 on the command that built the
    /// domain's backend and 0 afterwards — the measurement that proves the
    /// session backend is one per process.
    pub fn note_read(&mut self, domain: &'static str, built: u64) {
        if let Some(row) = self.counts.iter_mut().find(|r| r.domain == domain) {
            row.reads += 1;
            row.builds += built;
        } else {
            self.counts.push(ReadCount {
                domain,
                reads: 1,
                builds: built,
            });
        }
    }

    /// The counts as they stand (a copy: the REPL's `session reads` prints
    /// them between commands).
    pub fn reads(&self) -> Vec<ReadCount> {
        let mut rows = self.counts.clone();
        rows.sort_by(|a, b| a.domain.cmp(b.domain));
        rows
    }
}

/// The clock's timer ledger: this session's live timers plus the rows earlier
/// processes left in the state file.
#[derive(Debug, Default)]
pub struct ClockSession {
    /// The timers this session armed, in arming order.
    live: Vec<TimerRecord>,
    /// The state file the ledger is shared through (`None` ⇒ hermetic).
    file: Option<PathBuf>,
    /// This session's identity: `pid:armed_nanos`.
    session: String,
}

impl ClockSession {
    /// The hermetic ledger: this session's rows only, nothing persisted.
    pub fn hermetic() -> Self {
        ClockSession::default()
    }

    /// The persistent ledger: a state file under the shell's own state
    /// directory (`$SMART_STATE_DIR`, else the platform's state dir, else
    /// `~/.local/state/smart`), created on first write.
    pub fn persistent() -> Self {
        ClockSession {
            live: Vec::new(),
            file: state_file("timers.tsv"),
            session: session_id(),
        }
    }

    /// This session's identity (`pid:armed_nanos`).
    pub fn session(&self) -> &str {
        &self.session
    }

    /// Record a timer this session just armed, and share the row with the
    /// next process when this session is persistent. Hermetic sessions keep
    /// the row in memory only.
    pub fn record(&mut self, record: TimerRecord) {
        if let Some(file) = &self.file {
            append_row(file, &record);
        }
        self.live.push(record);
    }

    /// Retire a timer this session cancelled: the fd is gone, so the ledger
    /// must not name it as live any more.
    pub fn retire(&mut self, id: u64) {
        self.live.retain(|r| r.id != id);
    }

    /// The timers this session armed (live rows).
    pub fn live(&self) -> Vec<TimerRecord> {
        self.live.clone()
    }

    /// Every ledger row a fresh process should name: this session's live
    /// timers, then the shared file's rows whose owner is another process —
    /// each measured against `/proc` (the owner is alive or it is not) and
    /// this session's own rows are never duplicated.
    pub fn rows(&self) -> Vec<TimerRow> {
        let mut rows: Vec<TimerRow> = self
            .live
            .iter()
            .cloned()
            .map(|record| TimerRow {
                record,
                ownership: TimerOwnership::Live,
            })
            .collect();
        for record in read_ledger(self.file.as_deref()) {
            if record.session == self.session {
                continue;
            }
            let ownership = if process_alive(record.pid) {
                TimerOwnership::Owned
            } else {
                TimerOwnership::Gone
            };
            rows.push(TimerRow { record, ownership });
        }
        rows
    }
}

/// One directory a session subscribed to, as the ledger's own row.
///
/// An inotify watch is a kernel object owned by the fd that armed it, exactly
/// as a timerfd is: no other process can pull its queue, and the queue is
/// dropped when the fd is. So the ledger records the FACT of the subscription
/// (who armed it, over what, when) and never the events — an event belongs to
/// the process that pulled it, and a stale row is named `gone` rather than
/// replayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchRecord {
    pub pid: u32,
    pub path: String,
    /// When the subscription was armed, in the engine's own epoch nanoseconds.
    pub started_at_nanos: u128,
    /// The owning session, as `pid:started_nanos`.
    pub session: String,
}

/// Where a watch row stands when a fresh process reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchOwnership {
    /// This session armed it and it is open right now.
    Live,
    /// Another process armed it and is still running.
    Owned,
    /// The owner exited: the inotify fd died with it.
    Gone,
}

/// A ledger row as `files watches` renders it: the record plus the measured
/// ownership, resolved against the host's own `/proc`.
#[derive(Debug, Clone)]
pub struct WatchRow {
    pub record: WatchRecord,
    pub ownership: WatchOwnership,
}

/// The files domain's watch ledger, the shape [`ClockSession`] uses for timers:
/// this session's live subscriptions plus the rows earlier processes left.
#[derive(Debug, Default)]
pub struct WatchSession {
    live: Vec<WatchRecord>,
    file: Option<PathBuf>,
    session: String,
}

impl WatchSession {
    /// The hermetic ledger: this session's rows only, nothing persisted.
    pub fn hermetic() -> Self {
        WatchSession::default()
    }

    /// The persistent ledger: `watches.tsv` beside the timer ledger.
    pub fn persistent() -> Self {
        WatchSession {
            live: Vec::new(),
            file: state_file("watches.tsv"),
            session: session_id(),
        }
    }

    /// This session's identity (`pid:started_nanos`).
    pub fn session(&self) -> &str {
        &self.session
    }

    /// Record a subscription this session just armed. A path this session is
    /// already watching is re-stamped rather than duplicated: one live row per
    /// path per session, because one process holds at most one watch of it.
    pub fn begin(&mut self, path: &str, started_at_nanos: u128) {
        self.live.retain(|r| r.path != path);
        let record = WatchRecord {
            pid: std::process::id(),
            path: path.to_string(),
            started_at_nanos,
            session: self.session.clone(),
        };
        // A path with a tab or newline would end the row early: it stays in
        // this session's memory and is not shared, rather than corrupting the
        // shared file for every later reader.
        if let Some(file) = &self.file {
            if !record.path.contains('\t') && !record.path.contains('\n') {
                append_tab_row(
                    file,
                    &[
                        &record.pid.to_string(),
                        &record.path,
                        &record.started_at_nanos.to_string(),
                        &record.session,
                    ],
                );
            }
        }
        self.live.push(record);
    }

    /// Retire a subscription: the fd is gone, so the ledger must not name it as
    /// live any more.
    pub fn retire(&mut self, path: &str) {
        self.live.retain(|r| r.path != path);
    }

    /// The subscriptions a fresh process should name: this session's live rows
    /// first, then the shared file's rows whose owner is another process, each
    /// measured against `/proc`.
    pub fn rows(&self) -> Vec<WatchRow> {
        let mut rows: Vec<WatchRow> = self
            .live
            .iter()
            .cloned()
            .map(|record| WatchRow {
                record,
                ownership: WatchOwnership::Live,
            })
            .collect();
        for record in read_watch_ledger(self.file.as_deref()) {
            if record.session == self.session {
                continue;
            }
            let ownership = if process_alive(record.pid) {
                WatchOwnership::Owned
            } else {
                WatchOwnership::Gone
            };
            rows.push(WatchRow { record, ownership });
        }
        rows
    }
}

/// Every watch row the shared ledger holds, oldest first.
fn read_watch_ledger(file: Option<&std::path::Path>) -> Vec<WatchRecord> {
    let Some(file) = file else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            Some(WatchRecord {
                pid: f.next()?.parse().ok()?,
                path: f.next()?.to_string(),
                started_at_nanos: f.next()?.parse().ok()?,
                session: f.next()?.to_string(),
            })
        })
        .collect()
}

/// The shell's own state directory: the operator's override first, then the
/// platform's, then the home-relative convention. Read-only measurement — a
/// missing directory is simply "no shared ledger", not an error.
fn state_file(name: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SMART_STATE_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join(name));
    }
    let dir = dirs::state_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))?;
    Some(dir.join("smart").join(name))
}

/// This session's identity: the pid plus the moment the ledger was opened, so
/// two sessions from the same binary are never conflated.
fn session_id() -> String {
    let pid = std::process::id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{pid}:{now}")
}

/// One ledger line: `pid \t id \t label \t after_ms \t interval_ms \t clock
/// \t armed_at \t session`. The label is the only free text, and it is
/// percent-free: a tab or newline would end the row, so it is rejected there
/// rather than corrupting the file.
fn append_row(file: &PathBuf, record: &TimerRecord) {
    if record.label.contains('\t') || record.label.contains('\n') {
        return;
    }
    append_tab_row(
        file,
        &[
            &record.pid.to_string(),
            &record.id.to_string(),
            &record.label,
            &record.after_ms.to_string(),
            &record.interval_ms.to_string(),
            &record.clock,
            &record.armed_at_nanos.to_string(),
            &record.session,
        ],
    );
}

/// Append one tab-separated row to a shared ledger, creating the file's parent
/// on the way. A write that fails (a read-only state directory, a full disk)
/// is simply "no shared ledger" — the session still works, it just cannot tell
/// the next process anything.
fn append_tab_row(file: &PathBuf, columns: &[&str]) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    use std::io::Write;
    if let Ok(mut handle) = std::fs::OpenOptions::new().append(true).create(true).open(file) {
        let _ = writeln!(handle, "{}", columns.join("\t"));
    }
}

/// Every row the shared ledger holds, oldest first. A malformed line is
/// skipped rather than guessed at.
fn read_ledger(file: Option<&std::path::Path>) -> Vec<TimerRecord> {
    let Some(file) = file else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    text.lines().filter_map(|line| parse_row(line)).collect()
}

fn parse_row(line: &str) -> Option<TimerRecord> {
    let mut f = line.split('\t');
    Some(TimerRecord {
        pid: f.next()?.parse().ok()?,
        id: f.next()?.parse().ok()?,
        label: f.next()?.to_string(),
        after_ms: f.next()?.parse().ok()?,
        interval_ms: f.next()?.parse().ok()?,
        clock: f.next()?.to_string(),
        armed_at_nanos: f.next()?.parse().ok()?,
        session: f.next()?.to_string(),
    })
}

/// Whether a pid is still running, measured the way the host answers it: the
/// kernel's own `/proc` entry. A read, never a signal (charter law #8).
fn process_alive(pid: u32) -> bool {
    pid != std::process::id() && std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hermetic session is hermetic: no state file is resolved, so a
    /// recorded row goes nowhere but this process's memory.
    #[test]
    fn the_hermetic_session_never_resolves_a_state_file() {
        let clock = ClockSession::hermetic();
        assert!(clock.file.is_none());
        assert!(clock.rows().is_empty());
        let watches = WatchSession::hermetic();
        assert!(watches.file.is_none());
        assert!(watches.rows().is_empty());
        assert!(SessionState::hermetic().reads().is_empty());
    }

    /// A read count is counted per domain, and the build count is 1 on the
    /// command that built the backend and 0 on every later one: that pair is
    /// the measurement that the per-invocation-backend rot is gone.
    #[test]
    fn the_read_counts_prove_one_backend_per_domain() {
        let mut state = SessionState::hermetic();
        state.note_read("process", 1);
        state.note_read("process", 0);
        state.note_read("process", 0);
        state.note_read("clock", 1);
        let rows = state.reads();
        assert_eq!(rows.len(), 2);
        let process = rows.iter().find(|r| r.domain == "process").expect("process");
        assert_eq!(process.reads, 3);
        assert_eq!(process.builds, 1, "one build, however many reads");
        let clock = rows.iter().find(|r| r.domain == "clock").expect("clock");
        assert_eq!((clock.reads, clock.builds), (1, 1));
    }

    /// The ledger's tab-separated form round-trips: what `append_row` writes is
    /// what `parse_row` reads back, field for field.
    #[test]
    fn a_timer_row_round_trips_through_the_ledger_form() {
        let record = TimerRecord {
            pid: 4242,
            id: 7,
            label: "nightly sweep".to_string(),
            after_ms: 250,
            interval_ms: 1000,
            clock: "monotonic".to_string(),
            armed_at_nanos: 1_772_000_000_000_000_000,
            session: "4242:99".to_string(),
        };
        let dir = std::env::temp_dir().join("smart-shell-session-test");
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let file = dir.join(format!("timers-{}.tsv", std::process::id()));
        append_row(&file, &record);
        let rows = read_ledger(Some(&file));
        let back = rows.iter().find(|r| r.session == record.session).expect("the row");
        assert_eq!(&record, back);
        let _ = std::fs::remove_file(&file);
    }

    /// A label carrying the row separator would end the line early, so it is
    /// kept out of the shared file rather than corrupting it for later readers.
    #[test]
    fn a_label_with_a_separator_is_not_shared() {
        let dir = std::env::temp_dir().join("smart-shell-session-test");
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let file = dir.join(format!("timers-bad-{}.tsv", std::process::id()));
        append_row(
            &file,
            &TimerRecord {
                pid: 1,
                id: 1,
                label: "a\tlabel".to_string(),
                after_ms: 0,
                interval_ms: 0,
                clock: "monotonic".to_string(),
                armed_at_nanos: 1,
                session: "1:1".to_string(),
            },
        );
        assert!(!file.exists() || std::fs::read_to_string(&file).expect("read").is_empty());
        let _ = std::fs::remove_file(&file);
    }

    /// Ownership is measured against the host's own `/proc`, never assumed: a
    /// pid that is not running is GONE, and the caller's own pid is never a
    /// sibling owner (its rows are the session's own, answered separately).
    #[test]
    fn ownership_is_measured_against_the_hosts_own_proc() {
        assert!(
            !process_alive(std::process::id()),
            "the caller's own pid is never measured as a sibling's"
        );
        assert!(!process_alive(0), "pid 0 is not a process this uid owns");
        // A pid unused on this boot: /proc carries no entry for it.
        assert!(!process_alive(4_194_303), "an unused pid is not alive");
    }

    /// A watch row's lifetime is the command's: `begin` records it, `retire`
    /// takes it back, and a path watched twice leaves one row.
    #[test]
    fn a_watch_rows_lifetime_is_the_command_holding_the_fd() {
        let mut watches = WatchSession::hermetic();
        watches.begin("/tmp/smart-shell-watch", 10);
        watches.begin("/tmp/smart-shell-watch", 20);
        assert_eq!(watches.rows().len(), 1, "one row per path per session");
        assert_eq!(watches.rows()[0].ownership, WatchOwnership::Live);
        watches.retire("/tmp/smart-shell-watch");
        assert!(watches.rows().is_empty(), "retired is not live");
    }

    /// The board, not a hand-kept string, is what `help` lists: the nine live
    /// domains answer from the migration queue's own status, so a domain that
    /// lands in the engine appears in `help` the same commit.
    #[test]
    fn the_board_names_every_live_domain() {
        let live = crate::migrate::live_domains();
        let mut expected = [
            "network",
            "clipboard",
            "service",
            "files",
            "process",
            "time",
            "settings",
            "logging",
            "clock",
        ];
        expected.sort_unstable();
        let mut seen = live.clone();
        seen.sort_unstable();
        assert_eq!(
            seen,
            expected,
            "help lists what the board says is live — all nine, not a hardcoded three"
        );
    }

    /// The confirmation standard, at the door that needs no terminal: a read
    /// verb is never asked anything, and `--force` is the documented
    /// acceptance that skips the prompt.
    #[test]
    fn the_write_gate_never_asks_a_read_verb() {
        let interactive = crate::shell::WriteGate::interactive(false, false);
        let forced = crate::shell::WriteGate::interactive(true, false);
        let scripted = crate::shell::WriteGate::scripted(false, false);
        assert_eq!(crate::shell::accept_write(None, "", interactive), Ok(false));
        assert_eq!(
            crate::shell::accept_write(None, "some context", interactive),
            Ok(false),
            "a read verb is never asked anything"
        );
        assert_eq!(
            crate::shell::accept_write(Some(("delete".into(), "/tmp/x".into())), "", forced),
            Ok(true),
            "--force accepts the write without a prompt"
        );
        // Script mode has no operator to ask: it refuses, deterministically,
        // and never reads the caller's program as an answer.
        let refusal = crate::shell::accept_write(
            Some(("delete".into(), "/tmp/x".into())),
            "regular, 0 B",
            scripted,
        );
        let message = refusal.expect_err("script mode refuses").to_string();
        assert!(message.contains("script mode"), "{message}");
        assert!(message.contains("--force"), "--force is named: {message}");
    }
}
