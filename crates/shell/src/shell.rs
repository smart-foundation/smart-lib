//! The Smart Shell's core: the REPL, the AST, and the tree-walking
//! evaluator — the original shell's engine-agnostic half, salvaged.
//!
//! What was repaired while porting (the original's half-real parts):
//!
//! * **tab completion is wired** — the original built a `Completer` and never
//!   gave it to rustyline; [`run_shell`] installs it as a rustyline `Helper`.
//! * **`elif`/`else` dispatch** — the original's parser never populated the
//!   elif/else branches (it built `If { elifs: vec![], else_body: vec![] }`),
//!   so they parsed and silently did nothing. The block parser here fills
//!   them, and the evaluator (which was already correct) dispatches them.
//! * **`watch`** — the original slept one second and re-printed a command.
//!   For network it is now the engine store's real bounded event stream:
//!   subscribe, pull, and the store revision is the event.
//! * **`apply`/`export`** — the original applied a hard-coded `audio.volume`
//!   and ignored its path. Both are real over the engine now, with a real
//!   `[network]` section.
//! * **the dead second REPL and the dead `Expr::Pipe` branch are gone** — the
//!   pipe expression is live: `var = @network/interfaces | count` evaluates
//!   through the same evaluator as a command pipe.
//!
//! The aesthetic is spec: box-drawing, `◉/○`, `↑↓`, aligned tables.

#[cfg(any(feature = "domain-network", feature = "domain-clipboard"))]
use clap::Parser as _;
use std::collections::HashMap;

use crate::alias;
#[cfg(feature = "domain-network")]
use crate::confirm::confirm_host_write;
use crate::color::ColorExt;
#[cfg(feature = "domain-network")]
use crate::complete::Completer;
use crate::migrate;
use crate::pipe::{self, PipeOp};
use crate::selector::{self, Value};
use crate::widgets::{Cell, Sgr, Table};
use crate::{OutputFormat, SmartError, SmartResult};

use crate::domains::network::Backend;
#[cfg(feature = "domain-clipboard")]
use crate::domains::clipboard::Backend as ClipboardBackend;
#[cfg(feature = "domain-service")]
use crate::domains::service::Backend as ServiceBackend;

/// How a mutating verb may be accepted, and by whom.
///
/// The founder's standard answers the same three ways from both doors (the
/// REPL and the one-shot binary), and this is everything an answer depends on:
/// who is asking, whether they already accepted, and whether stdin belongs to
/// them at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteGate {
    /// The operator said `--force` on the command line: the prompt is skipped.
    pub force: bool,
    /// The operator said `--dry-run`: the plan is printed and nothing is
    /// written.
    pub dry_run: bool,
    /// The caller is driving stdin itself — the REPL's script mode, where
    /// stdin is the PROGRAM and never the answer channel. A prompt here would
    /// block forever against the caller's own reader, so this caller is told
    /// it has no operator to ask instead of hanging.
    pub script_mode: bool,
}

impl WriteGate {
    /// The interactive gate: an operator answers the prompt, unless they
    /// already accepted with `--force` / `--dry-run`.
    pub fn interactive(force: bool, dry_run: bool) -> Self {
        WriteGate {
            force,
            dry_run,
            script_mode: false,
        }
    }

    /// The script-mode gate: stdin is the caller's, so a destructive verb
    /// refuses rather than reading the program as an answer.
    pub fn scripted(force: bool, dry_run: bool) -> Self {
        WriteGate {
            force,
            dry_run,
            script_mode: true,
        }
    }
}

/// One line of gloss per LIVE domain, in the words the REPL's `help` prints
/// under the board's list. The board (`migrate::live_domains`) decides WHICH
/// domains are live and in what order; this decides only what each is called.
/// A live domain missing here still appears — it simply gets the board's
/// default gloss — so the list can never be shortened by this table going
/// stale, which was the old failure.
const LIVE_DOMAIN_GLOSS: &[(&str, &str)] = &[
    ("network", "interfaces, wifi, radios — netlink + NetworkManager"),
    ("clipboard", "the native selection: get/set/history/convert (writes gate-routed)"),
    ("service", "the supervisor's services: state, health, deps, watch (writes gated)"),
    ("files", "the host's files: ls/find/tree/stat/cat/du/watch (writes gated)"),
    ("process", "/proc: every process, inspect, tree, groups, caps (writes gated)"),
    ("settings", "the host's schemas, keys and values, live (writes gated)"),
    ("logging", "the host's sinks and logs: sinks/domains/tail/entries/follow"),
    ("clock", "clocks, timerfds and alarms — the session's ledger keeps them live"),
    ("time", "wall time, TAI, the contract's calendars and their arithmetic"),
    ("audio", "the host's sinks, sources and streams (writes gated)"),
    ("media", "the session's MPRIS players (writes gated)"),
    ("power", "the host's battery and profiles (writes gated)"),
    ("database", "the engine's namespaced key/value store (writes gated)"),
    ("queue", "the engine's FIFO store (writes gated)"),
    ("appindex", "the host's desktop-entry corpus: list, search, stats"),
    ("manifest", "the engine's manifest store: resolve, validate (writes gated)"),
    ("vcard", "RFC 6350 cards: parse, format, import, export (writes gated)"),
    ("content", "the engine's content store: query, get (writes gated)"),
];

#[derive(Debug, Clone)]
pub enum ShellCommand {
    Exec(String),
    Assign { name: String, value: Expr },
    If {
        condition: Expr,
        then_body: Vec<ShellCommand>,
        elifs: Vec<(Expr, Vec<ShellCommand>)>,
        else_body: Vec<ShellCommand>,
    },
    For {
        var: String,
        iter: Expr,
        body: Vec<ShellCommand>,
    },
    Fn {
        name: String,
        params: Vec<String>,
        body: Vec<ShellCommand>,
    },
    Watch {
        domain: String,
        args: Vec<String>,
    },
    Apply {
        path: String,
    },
    Export {
        path: Option<String>,
    },
    Help {
        domain: Option<String>,
    },
    /// The session's own surface: the per-domain read counts that prove the
    /// session backend is one per process (`session reads`).
    Session {
        action: String,
    },
    Migrate,
    Exit,
    Empty,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Selector(String),
    Literal(Value),
    Variable(String),
    Comparison {
        left: Box<Expr>,
        op: CompOp,
        right: Box<Expr>,
    },
    Pipe {
        expr: Box<Expr>,
        ops: Vec<PipeOp>,
    },
    FunctionCall {
        name: String,
        args: Vec<Expr>,
    },
}

#[derive(Debug, Clone)]
pub enum CompOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
}

pub struct Shell {
    pub variables: HashMap<String, Value>,
    pub functions: HashMap<String, (Vec<String>, Vec<ShellCommand>)>,
    #[cfg(feature = "domain-network")]
    pub backend: Box<Backend>,
    #[cfg(feature = "domain-clipboard")]
    pub clipboard: Box<ClipboardBackend>,
    #[cfg(feature = "domain-service")]
    pub service: Box<ServiceBackend>,
    /// The session: per-domain read counts and the clock-timer ledger. Built
    /// once per shell process — the REPL's every line, a `-c` line and a piped
    /// script line all drive the same backends and the same ledger, which is
    /// what makes `clock timer create → list → check` compose.
    pub session: crate::session::SessionState,
    /// How a mutating verb may be accepted: `--force`, `--dry-run`, and
    /// whether stdin is this caller's program (the REPL's script mode).
    pub gate: WriteGate,
    // ── the session-scoped domain backends ──────────────────────────────
    // Built on first use and memoized for the life of the process. The rot
    // this replaced: every command constructed a fresh
    // `Linux…Backend::new()`, so state a verb created died with its own
    // command — create→list→check could never meet, and a subscription was
    // cancelled by the line's end. `None` until the domain's first command;
    // the binary prefers connecting the clipboard/service backends up front
    // (`connect_clipboard` / `connect_service`).
    #[cfg(feature = "domain-files")]
    files_backend: Option<Box<crate::domains::files::Backend>>,
    #[cfg(feature = "domain-process")]
    process_backend: Option<Box<crate::domains::process::Backend>>,
    #[cfg(feature = "domain-settings")]
    settings_backend: Option<Box<crate::domains::settings::Backend>>,
    #[cfg(feature = "domain-logging")]
    logging_backend: Option<Box<crate::domains::logging::Backend>>,
    #[cfg(feature = "domain-clock")]
    clock_backend: Option<Box<crate::domains::clock::Backend>>,
    #[cfg(feature = "domain-time")]
    time_backend: Option<Box<crate::domains::time::Backend>>,
    #[cfg(feature = "domain-audio")]
    audio_backend: Option<Box<crate::domains::audio::Backend>>,
    #[cfg(feature = "domain-media")]
    media_backend: Option<Box<crate::domains::media::Backend>>,
    #[cfg(feature = "domain-power")]
    power_backend: Option<Box<crate::domains::power::Backend>>,
    // ── the Batch 6 six ───────────────────────────────────────────────────
    // The engine's own stores: the namespaced key/value store, the FIFO, the
    // desktop-entry index, the manifest store, the vCard store and the content
    // store. They live in the engine's process memory, so the session keeps
    // ONE backend per domain — a write a line makes is a read the next line
    // can see.
    #[cfg(feature = "domain-database")]
    database_backend: Option<Box<crate::domains::database::Backend>>,
    #[cfg(feature = "domain-queue")]
    queue_backend: Option<Box<crate::domains::queue::Backend>>,
    #[cfg(feature = "domain-appindex")]
    appindex_backend: Option<Box<crate::domains::appindex::Backend>>,
    #[cfg(feature = "domain-manifest")]
    manifest_backend: Option<Box<crate::domains::manifest::Backend>>,
    #[cfg(feature = "domain-vcard")]
    vcard_backend: Option<Box<crate::domains::vcard::Backend>>,
    #[cfg(feature = "domain-content")]
    content_backend: Option<Box<crate::domains::content::Backend>>,
}

impl Shell {
    pub fn new(backend: Box<Backend>) -> Self {
        Shell {
            variables: HashMap::new(),
            functions: HashMap::new(),
            #[cfg(feature = "domain-network")]
            backend,
            #[cfg(feature = "domain-clipboard")]
            clipboard: Box::new(smart_lib::platforms::linux::clipboard::LinuxClipboardBackend::solid()),
            #[cfg(feature = "domain-service")]
            service: Box::new(smart_lib::platforms::linux::service::LinuxServiceBackend::native_connect()),
            session: crate::session::SessionState::hermetic(),
            gate: WriteGate::default(),
            #[cfg(feature = "domain-files")]
            files_backend: None,
            #[cfg(feature = "domain-process")]
            process_backend: None,
            #[cfg(feature = "domain-settings")]
            settings_backend: None,
            #[cfg(feature = "domain-logging")]
            logging_backend: None,
            #[cfg(feature = "domain-clock")]
            clock_backend: None,
            #[cfg(feature = "domain-time")]
            time_backend: None,
            #[cfg(feature = "domain-audio")]
            audio_backend: None,
            #[cfg(feature = "domain-media")]
            media_backend: None,
            #[cfg(feature = "domain-power")]
            power_backend: None,
            #[cfg(feature = "domain-database")]
            database_backend: None,
            #[cfg(feature = "domain-queue")]
            queue_backend: None,
            #[cfg(feature = "domain-appindex")]
            appindex_backend: None,
            #[cfg(feature = "domain-manifest")]
            manifest_backend: None,
            #[cfg(feature = "domain-vcard")]
            vcard_backend: None,
            #[cfg(feature = "domain-content")]
            content_backend: None,
        }
    }

    /// Share the session's state with the next process: the clock-timer ledger
    /// is written to (and read from) a small state file, so `clock timer list`
    /// in a fresh invocation can name the timers a live sibling session holds
    /// and the ones whose owner exited. The binary calls this; the library's
    /// `Shell::new` stays hermetic (no state file, nothing written).
    pub fn enable_state_persistence(&mut self) {
        self.session = crate::session::SessionState::persistent();
    }

    /// Record `--force` for this session: the explicit acceptance that skips
    /// the interactive prompt (documented on the flag; a script never gets a
    /// fabricated yes).
    pub fn set_force(&mut self, force: bool) {
        self.gate.force = force;
    }

    /// Record `--dry-run` for this session: every destructive verb prints its
    /// plan and performs no write.
    pub fn set_dry_run(&mut self, dry_run: bool) {
        self.gate.dry_run = dry_run;
    }

    /// Mark this session as driving stdin itself (the REPL's script mode):
    /// a destructive verb then refuses instead of reading the program as an
    /// answer, which is also what keeps it from hanging.
    pub fn set_script_mode(&mut self, script_mode: bool) {
        self.gate.script_mode = script_mode;
    }

    /// Install the whole gate at once (the binary's entry).
    pub fn set_gate(&mut self, gate: WriteGate) {
        self.gate = gate;
    }

    /// The session's files backend, built once.
    #[cfg(feature = "domain-files")]
    pub fn files(&mut self) -> &crate::domains::files::Backend {
        let built = self.files_backend.is_none() as u64;
        self.session.note_read("files", built);
        if self.files_backend.is_none() {
            self.files_backend = Some(Box::new(
                smart_lib::platforms::linux::files::LinuxFilesBackend::new(),
            ) as Box<crate::domains::files::Backend>);
        }
        self.files_backend.as_deref().expect("built just above")
    }

    /// Install the session's files backend (the hermetic test seam).
    #[cfg(feature = "domain-files")]
    pub fn set_files(&mut self, backend: Box<crate::domains::files::Backend>) {
        self.files_backend = Some(backend);
    }

    /// The session's process backend, built once.
    #[cfg(feature = "domain-process")]
    pub fn process(&mut self) -> &crate::domains::process::Backend {
        let built = self.process_backend.is_none() as u64;
        self.session.note_read("process", built);
        if self.process_backend.is_none() {
            self.process_backend = Some(Box::new(
                smart_lib::platforms::linux::process::LinuxProcessBackend::new(),
            ) as Box<crate::domains::process::Backend>);
        }
        self.process_backend.as_deref().expect("built just above")
    }

    /// Install the session's process backend (the hermetic test seam).
    #[cfg(feature = "domain-process")]
    pub fn set_process(&mut self, backend: Box<crate::domains::process::Backend>) {
        self.process_backend = Some(backend);
    }

    /// The session's settings backend, built once.
    #[cfg(feature = "domain-settings")]
    pub fn settings(&mut self) -> &crate::domains::settings::Backend {
        let built = self.settings_backend.is_none() as u64;
        self.session.note_read("settings", built);
        if self.settings_backend.is_none() {
            self.settings_backend = Some(Box::new(
                smart_lib::platforms::linux::settings::LinuxSettingsBackend::new(),
            ) as Box<crate::domains::settings::Backend>);
        }
        self.settings_backend.as_deref().expect("built just above")
    }

    /// Install the session's settings backend (the hermetic test seam).
    #[cfg(feature = "domain-settings")]
    pub fn set_settings(&mut self, backend: Box<crate::domains::settings::Backend>) {
        self.settings_backend = Some(backend);
    }

    /// The session's logging backend, built once.
    #[cfg(feature = "domain-logging")]
    pub fn logging(&mut self) -> &crate::domains::logging::Backend {
        let built = self.logging_backend.is_none() as u64;
        self.session.note_read("logging", built);
        if self.logging_backend.is_none() {
            self.logging_backend = Some(Box::new(
                smart_lib::platforms::linux::logging::LinuxLoggingBackend::new(),
            ) as Box<crate::domains::logging::Backend>);
        }
        self.logging_backend.as_deref().expect("built just above")
    }

    /// Install the session's logging backend (the hermetic test seam).
    #[cfg(feature = "domain-logging")]
    pub fn set_logging(&mut self, backend: Box<crate::domains::logging::Backend>) {
        self.logging_backend = Some(backend);
    }

    /// The session's clock backend, built once: the timers and alarms this
    /// session arms live here for the whole process, which is what lets
    /// `timer create → list → check` meet.
    #[cfg(feature = "domain-clock")]
    pub fn clock(&mut self) -> &crate::domains::clock::Backend {
        let built = self.clock_backend.is_none() as u64;
        self.session.note_read("clock", built);
        if self.clock_backend.is_none() {
            self.clock_backend = Some(Box::new(
                smart_lib::platforms::linux::clock::LinuxClockBackend::new(),
            ) as Box<crate::domains::clock::Backend>);
        }
        self.clock_backend.as_deref().expect("built just above")
    }

    /// Install the session's clock backend (the hermetic test seam).
    #[cfg(feature = "domain-clock")]
    pub fn set_clock(&mut self, backend: Box<crate::domains::clock::Backend>) {
        self.clock_backend = Some(backend);
    }

    /// The session's time backend, built once.
    #[cfg(feature = "domain-time")]
    pub fn time(&mut self) -> &crate::domains::time::Backend {
        let built = self.time_backend.is_none() as u64;
        self.session.note_read("time", built);
        if self.time_backend.is_none() {
            self.time_backend = Some(Box::new(
                smart_lib::platforms::linux::time::LinuxTimeBackend::new(),
            ) as Box<crate::domains::time::Backend>);
        }
        self.time_backend.as_deref().expect("built just above")
    }

    /// Install the session's time backend (the hermetic test seam).
    #[cfg(feature = "domain-time")]
    pub fn set_time(&mut self, backend: Box<crate::domains::time::Backend>) {
        self.time_backend = Some(backend);
    }

    /// The session's audio backend, built once.
    #[cfg(feature = "domain-audio")]
    pub fn audio(&mut self) -> &crate::domains::audio::Backend {
        let built = self.audio_backend.is_none() as u64;
        self.session.note_read("audio", built);
        if self.audio_backend.is_none() {
            self.audio_backend = Some(Box::new(
                smart_lib::platforms::linux::audio::LinuxAudioBackend::new(),
            ) as Box<crate::domains::audio::Backend>);
        }
        self.audio_backend.as_deref().expect("built just above")
    }
    #[cfg(feature = "domain-audio")]
    pub fn set_audio(&mut self, backend: Box<crate::domains::audio::Backend>) {
        self.audio_backend = Some(backend);
    }

    /// The session's media backend, built once.
    #[cfg(feature = "domain-media")]
    pub fn media(&mut self) -> &crate::domains::media::Backend {
        let built = self.media_backend.is_none() as u64;
        self.session.note_read("media", built);
        if self.media_backend.is_none() {
            self.media_backend = Some(Box::new(
                smart_lib::platforms::linux::media::LinuxMediaBackend::new(),
            ) as Box<crate::domains::media::Backend>);
        }
        self.media_backend.as_deref().expect("built just above")
    }
    #[cfg(feature = "domain-media")]
    pub fn set_media(&mut self, backend: Box<crate::domains::media::Backend>) {
        self.media_backend = Some(backend);
    }

    /// The session's power backend, built once.
    #[cfg(feature = "domain-power")]
    pub fn power(&mut self) -> &crate::domains::power::Backend {
        let built = self.power_backend.is_none() as u64;
        self.session.note_read("power", built);
        if self.power_backend.is_none() {
            self.power_backend = Some(Box::new(
                smart_lib::platforms::linux::power::LinuxPowerBackend::new(),
            ) as Box<crate::domains::power::Backend>);
        }
        self.power_backend.as_deref().expect("built just above")
    }
    #[cfg(feature = "domain-power")]
    pub fn set_power(&mut self, backend: Box<crate::domains::power::Backend>) {
        self.power_backend = Some(backend);
    }

    /// The session's database backend, built once: the engine's own
    /// namespaced key/value store, which lives in this process's memory.
    #[cfg(feature = "domain-database")]
    pub fn database(&mut self) -> &crate::domains::database::Backend {
        let built = self.database_backend.is_none() as u64;
        self.session.note_read("database", built);
        if self.database_backend.is_none() {
            self.database_backend = Some(Box::new(
                smart_lib::platforms::linux::database::LinuxDatabaseBackend::new(),
            ) as Box<crate::domains::database::Backend>);
        }
        self.database_backend.as_deref().expect("built just above")
    }
    #[cfg(feature = "domain-database")]
    pub fn set_database(&mut self, backend: Box<crate::domains::database::Backend>) {
        self.database_backend = Some(backend);
    }

    /// The session's queue backend, built once: the engine's own FIFO store.
    #[cfg(feature = "domain-queue")]
    pub fn queue(&mut self) -> &crate::domains::queue::Backend {
        let built = self.queue_backend.is_none() as u64;
        self.session.note_read("queue", built);
        if self.queue_backend.is_none() {
            self.queue_backend = Some(Box::new(
                smart_lib::platforms::linux::queue::LinuxQueueBackend::new(),
            ) as Box<crate::domains::queue::Backend>);
        }
        self.queue_backend.as_deref().expect("built just above")
    }
    #[cfg(feature = "domain-queue")]
    pub fn set_queue(&mut self, backend: Box<crate::domains::queue::Backend>) {
        self.queue_backend = Some(backend);
    }

    /// The session's appindex backend, built once: the host's own
    /// freedesktop desktop-entry corpus, read natively.
    #[cfg(feature = "domain-appindex")]
    pub fn appindex(&mut self) -> &crate::domains::appindex::Backend {
        let built = self.appindex_backend.is_none() as u64;
        self.session.note_read("appindex", built);
        if self.appindex_backend.is_none() {
            self.appindex_backend = Some(Box::new(
                smart_lib::platforms::linux::appindex::LinuxAppindexBackend::new(),
            ) as Box<crate::domains::appindex::Backend>);
        }
        self.appindex_backend.as_deref().expect("built just above")
    }
    #[cfg(feature = "domain-appindex")]
    pub fn set_appindex(&mut self, backend: Box<crate::domains::appindex::Backend>) {
        self.appindex_backend = Some(backend);
    }

    /// The session's manifest backend, built once: the engine's own manifest
    /// store.
    #[cfg(feature = "domain-manifest")]
    pub fn manifest(&mut self) -> &crate::domains::manifest::Backend {
        let built = self.manifest_backend.is_none() as u64;
        self.session.note_read("manifest", built);
        if self.manifest_backend.is_none() {
            self.manifest_backend = Some(Box::new(
                smart_lib::platforms::linux::manifest::LinuxManifestBackend::new(),
            ) as Box<crate::domains::manifest::Backend>);
        }
        self.manifest_backend.as_deref().expect("built just above")
    }
    #[cfg(feature = "domain-manifest")]
    pub fn set_manifest(&mut self, backend: Box<crate::domains::manifest::Backend>) {
        self.manifest_backend = Some(backend);
    }

    /// The session's vcard backend, built once: the engine's own RFC 6350
    /// parser, formatter and contact store.
    #[cfg(feature = "domain-vcard")]
    pub fn vcard(&mut self) -> &crate::domains::vcard::Backend {
        let built = self.vcard_backend.is_none() as u64;
        self.session.note_read("vcard", built);
        if self.vcard_backend.is_none() {
            self.vcard_backend = Some(Box::new(
                smart_lib::platforms::linux::vcard::LinuxVcardBackend::new(),
            ) as Box<crate::domains::vcard::Backend>);
        }
        self.vcard_backend.as_deref().expect("built just above")
    }
    #[cfg(feature = "domain-vcard")]
    pub fn set_vcard(&mut self, backend: Box<crate::domains::vcard::Backend>) {
        self.vcard_backend = Some(backend);
    }

    /// The session's content backend, built once: the engine's own content
    /// store.
    #[cfg(feature = "domain-content")]
    pub fn content(&mut self) -> &crate::domains::content::Backend {
        let built = self.content_backend.is_none() as u64;
        self.session.note_read("content", built);
        if self.content_backend.is_none() {
            self.content_backend = Some(Box::new(
                smart_lib::platforms::linux::content::LinuxContentBackend::new(),
            ) as Box<crate::domains::content::Backend>);
        }
        self.content_backend.as_deref().expect("built just above")
    }
    #[cfg(feature = "domain-content")]
    pub fn set_content(&mut self, backend: Box<crate::domains::content::Backend>) {
        self.content_backend = Some(backend);
    }

    /// Build the shell with an explicitly-connected clipboard backend (the live
    /// X11/Wayland selection path when the host has a seat). Network is carried
    /// the same way as [`Shell::new`].
    #[cfg(feature = "domain-clipboard")]
    pub fn new_with_clipboard(
        backend: Box<Backend>,
        clipboard: Box<ClipboardBackend>,
    ) -> Self {
        Shell {
            variables: HashMap::new(),
            functions: HashMap::new(),
            #[cfg(feature = "domain-network")]
            backend,
            clipboard,
            #[cfg(feature = "domain-service")]
            service: Box::new(smart_lib::platforms::linux::service::LinuxServiceBackend::native_connect()),
            session: crate::session::SessionState::hermetic(),
            gate: WriteGate::default(),
            #[cfg(feature = "domain-files")]
            files_backend: None,
            #[cfg(feature = "domain-process")]
            process_backend: None,
            #[cfg(feature = "domain-settings")]
            settings_backend: None,
            #[cfg(feature = "domain-logging")]
            logging_backend: None,
            #[cfg(feature = "domain-clock")]
            clock_backend: None,
            #[cfg(feature = "domain-time")]
            time_backend: None,
            #[cfg(feature = "domain-audio")]
            audio_backend: None,
            #[cfg(feature = "domain-media")]
            media_backend: None,
            #[cfg(feature = "domain-power")]
            power_backend: None,
            #[cfg(feature = "domain-database")]
            database_backend: None,
            #[cfg(feature = "domain-queue")]
            queue_backend: None,
            #[cfg(feature = "domain-appindex")]
            appindex_backend: None,
            #[cfg(feature = "domain-manifest")]
            manifest_backend: None,
            #[cfg(feature = "domain-vcard")]
            vcard_backend: None,
            #[cfg(feature = "domain-content")]
            content_backend: None,
        }
    }

    /// Connect this shell's service backend to the host's supervisor (measured:
    /// runit on this box). Every read is the supervisor's own native path.
    #[cfg(feature = "domain-service")]
    pub fn connect_service(&mut self) {
        self.service =
            Box::new(smart_lib::platforms::linux::service::LinuxServiceBackend::native_connect());
    }

    /// Connect this shell's clipboard backend to the live native transport,
    /// falling back to the seat-less backend (honest Unavailable-with-reason)
    /// when the host has no display.
    #[cfg(feature = "domain-clipboard")]
    pub fn connect_clipboard(&mut self) {
        self.clipboard = match smart_lib::platforms::linux::clipboard::LinuxClipboardBackend::native_connect() {
            Ok(b) => Box::new(b),
            // A seat this backend does not serve (a Wayland seat) or a headless
            // host: the specific reason is surfaced, then the seat-less backend
            // answers every live read honestly rather than faking one.
            Err(e) => {
                eprintln!("clipboard: {e}");
                Box::new(smart_lib::platforms::linux::clipboard::LinuxClipboardBackend::solid())
            }
        };
    }

    // ── the evaluator ───────────────────────────────────────────────────────

    pub fn eval_expr(&mut self, expr: &Expr) -> SmartResult<Value> {
        match expr {
            Expr::Selector(s) => {
                let sel = selector::parse(s)?;
                // The domain is decided by the selector prefix; each real
                // domain builds its own tree from the engine's owned types, and
                // every other prefix answers honestly from the migration queue.
                let domain = sel.domain.as_str();
                #[cfg(feature = "domain-network")]
                if domain == "network" {
                    return Ok(selector::evaluate(&sel, self.backend.as_ref())?);
                }
                #[cfg(feature = "domain-clipboard")]
                if domain == "clipboard" {
                    let root = crate::domains::clipboard::selector_root(self.clipboard.as_ref())?;
                    return Ok(selector::resolve(root, &sel));
                }
                #[cfg(feature = "domain-service")]
                if domain == "service" {
                    let root = crate::domains::service::selector_root(self.service.as_ref())?;
                    return Ok(selector::resolve(root, &sel));
                }
                #[cfg(feature = "domain-files")]
                if domain == "files" {
                    return Ok(selector::resolve(
                        crate::domains::files::selector_root(self.files())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-process")]
                if domain == "process" {
                    return Ok(selector::resolve(
                        crate::domains::process::selector_root(self.process())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-settings")]
                if domain == "settings" {
                    return Ok(selector::resolve(
                        crate::domains::settings::selector_root(self.settings())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-logging")]
                if domain == "logging" {
                    return Ok(selector::resolve(
                        crate::domains::logging::selector_root(self.logging())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-clock")]
                if domain == "clock" {
                    return Ok(selector::resolve(
                        crate::domains::clock::selector_root(self.clock())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-time")]
                if domain == "time" {
                    return Ok(selector::resolve(
                        crate::domains::time::selector_root(self.time())?,
                        &sel,
                    ));
                }

                #[cfg(feature = "domain-audio")]
                if domain == "audio" {
                    return Ok(selector::resolve(
                        crate::domains::audio::selector_root(self.audio())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-media")]
                if domain == "media" {
                    return Ok(selector::resolve(
                        crate::domains::media::selector_root(self.media())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-power")]
                if domain == "power" {
                    return Ok(selector::resolve(
                        crate::domains::power::selector_root(self.power())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-database")]
                if domain == "database" {
                    return Ok(selector::resolve(
                        crate::domains::database::selector_root(self.database())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-queue")]
                if domain == "queue" {
                    return Ok(selector::resolve(
                        crate::domains::queue::selector_root(self.queue())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-appindex")]
                if domain == "appindex" {
                    return Ok(selector::resolve(
                        crate::domains::appindex::selector_root(self.appindex())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-manifest")]
                if domain == "manifest" {
                    return Ok(selector::resolve(
                        crate::domains::manifest::selector_root(self.manifest())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-vcard")]
                if domain == "vcard" {
                    return Ok(selector::resolve(
                        crate::domains::vcard::selector_root(self.vcard())?,
                        &sel,
                    ));
                }
                #[cfg(feature = "domain-content")]
                if domain == "content" {
                    return Ok(selector::resolve(
                        crate::domains::content::selector_root(self.content())?,
                        &sel,
                    ));
                }
                match domain {
                    #[cfg(not(feature = "domain-network"))]
                    "network" => {
                        return Err(SmartError::unavailable(
                            "the shell was built without the domain-network feature — the @network \
                             selector needs the engine's network domain",
                        ))
                    }
                    #[cfg(not(feature = "domain-clipboard"))]
                    "clipboard" => {
                        return Err(SmartError::unavailable(
                            "the shell was built without the domain-clipboard feature — the \
                             @clipboard selector needs the engine's clipboard domain",
                        ))
                    }
                    #[cfg(not(feature = "domain-service"))]
                    "service" => {
                        return Err(SmartError::unavailable(
                            "the shell was built without the domain-service feature — the \
                             @service selector needs the engine's service domain",
                        ))
                    }
                    #[cfg(not(feature = "domain-database"))]
                    "database" => {
                        return Err(SmartError::unavailable(
                            "the shell was built without the domain-database feature — the \
                             @database selector needs the engine's database domain",
                        ))
                    }
                    #[cfg(not(feature = "domain-queue"))]
                    "queue" => {
                        return Err(SmartError::unavailable(
                            "the shell was built without the domain-queue feature — the \
                             @queue selector needs the engine's queue domain",
                        ))
                    }
                    #[cfg(not(feature = "domain-appindex"))]
                    "appindex" => {
                        return Err(SmartError::unavailable(
                            "the shell was built without the domain-appindex feature — the \
                             @appindex selector needs the engine's appindex domain",
                        ))
                    }
                    #[cfg(not(feature = "domain-manifest"))]
                    "manifest" => {
                        return Err(SmartError::unavailable(
                            "the shell was built without the domain-manifest feature — the \
                             @manifest selector needs the engine's manifest domain",
                        ))
                    }
                    #[cfg(not(feature = "domain-vcard"))]
                    "vcard" => {
                        return Err(SmartError::unavailable(
                            "the shell was built without the domain-vcard feature — the \
                             @vcard selector needs the engine's vcard domain",
                        ))
                    }
                    #[cfg(not(feature = "domain-content"))]
                    "content" => {
                        return Err(SmartError::unavailable(
                            "the shell was built without the domain-content feature — the \
                             @content selector needs the engine's content domain",
                        ))
                    }
                    other => Err(SmartError::Failed(migrate::pending_reason(other))),
                }
            }
            Expr::Literal(v) => Ok(v.clone()),
            Expr::Variable(name) => Ok(self.variables.get(name).cloned().unwrap_or(Value::Null)),
            Expr::Comparison { left, op, right } => {
                let l = self.eval_expr(left)?;
                let r = self.eval_expr(right)?;
                let result = match op {
                    CompOp::Eq => l == r,
                    CompOp::Ne => l != r,
                    CompOp::Gt => l.as_f64().unwrap_or(0.0) > r.as_f64().unwrap_or(0.0),
                    CompOp::Lt => l.as_f64().unwrap_or(0.0) < r.as_f64().unwrap_or(0.0),
                    CompOp::Gte => l.as_f64().unwrap_or(0.0) >= r.as_f64().unwrap_or(0.0),
                    CompOp::Lte => l.as_f64().unwrap_or(0.0) <= r.as_f64().unwrap_or(0.0),
                };
                Ok(Value::Bool(result))
            }
            Expr::Pipe { expr, ops } => {
                let mut current = self.eval_expr(expr)?;
                for op in ops {
                    current = pipe::apply_pipe(current, op)?;
                }
                Ok(current)
            }
            Expr::FunctionCall { name, args } => {
                let arg_vals: SmartResult<Vec<Value>> =
                    args.iter().map(|a| self.eval_expr(a)).collect();
                let arg_vals = arg_vals?;
                self.call_function(name, &arg_vals)
            }
        }
    }

    fn call_function(&mut self, name: &str, args: &[Value]) -> SmartResult<Value> {
        if let Some((params, body)) = self.functions.get(name).cloned() {
            let old_vars = self.variables.clone();
            for (param, arg) in params.iter().zip(args) {
                self.variables.insert(param.clone(), arg.clone());
            }
            let mut result = Value::Null;
            for cmd in &body {
                result = self.exec_cmd(cmd)?;
            }
            self.variables = old_vars;
            Ok(result)
        } else {
            Err(SmartError::failed(format!("Unknown function: {name}")))
        }
    }

    pub fn exec_cmd(&mut self, cmd: &ShellCommand) -> SmartResult<Value> {
        match cmd {
            ShellCommand::Empty => Ok(Value::Null),
            ShellCommand::Exec(input) => self.execute_command(input),
            ShellCommand::Assign { name, value } => {
                let val = self.eval_expr(value)?;
                self.variables.insert(name.clone(), val.clone());
                Ok(val)
            }
            ShellCommand::If {
                condition,
                then_body,
                elifs,
                else_body,
            } => {
                // A conditional yields its taken branch's last value, so a
                // branch's assignment is observable (and an empty branch is
                // Null, never a guess).
                if self.eval_expr(condition)?.is_truthy() {
                    let mut last = Value::Null;
                    for cmd in then_body {
                        last = self.exec_cmd(cmd)?;
                    }
                    Ok(last)
                } else {
                    for (elif_cond, elif_body) in elifs {
                        if self.eval_expr(elif_cond)?.is_truthy() {
                            let mut last = Value::Null;
                            for cmd in elif_body {
                                last = self.exec_cmd(cmd)?;
                            }
                            return Ok(last);
                        }
                    }
                    let mut last = Value::Null;
                    for cmd in else_body {
                        last = self.exec_cmd(cmd)?;
                    }
                    Ok(last)
                }
            }
            ShellCommand::For { var, iter, body } => {
                let iter_val = self.eval_expr(iter)?;
                if let Value::List(items) = iter_val {
                    for item in items {
                        self.variables.insert(var.clone(), item);
                        for cmd in body {
                            self.exec_cmd(cmd)?;
                        }
                    }
                }
                Ok(Value::Null)
            }
            ShellCommand::Fn { name, params, body } => {
                self.functions
                    .insert(name.clone(), (params.clone(), body.clone()));
                Ok(Value::Null)
            }
            ShellCommand::Watch { domain, args } => self.execute_watch(domain, args),
            ShellCommand::Apply { path } => self.execute_apply(path),
            ShellCommand::Export { path } => self.execute_export(path.as_deref()),
            ShellCommand::Help { domain } => {
                self.execute_help(domain.as_deref());
                Ok(Value::Null)
            }
            ShellCommand::Session { action } => self.execute_session(action),
            ShellCommand::Migrate => {
                print!("{}", migrate::render_queue(&OutputFormat::Human));
                Ok(Value::Null)
            }
            ShellCommand::Exit => std::process::exit(0),
        }
    }

    // ── command dispatch ────────────────────────────────────────────────────

    fn execute_command(&mut self, input: &str) -> SmartResult<Value> {
        let input = input.trim();
        if input.is_empty() {
            return Ok(Value::Null);
        }

        if input.starts_with('@') {
            let expr = Shell::parse_expr(input);
            return self.eval_expr(&expr);
        }

        if input.contains('|') {
            // The pipe path: the command's VALUE flows through the ops — not
            // its printed rendering — and the final value is what the caller
            // renders. This is what makes `network | where type == wifi` a
            // filter rather than a filter of nothing.
            let (cmd_part, ops_str) = input.split_at(
                input.find('|').ok_or_else(|| SmartError::invalid_argument("pipe without a bar"))?,
            );
            let ops = pipe::parse_pipe_chain(ops_str)?;
            let value = self.execute_single(cmd_part.trim(), false)?;
            return Ok(pipe::apply_pipe_chain(value, &ops)?);
        }

        self.execute_single(input, true)
    }

    /// Evaluate one command. `render` prints the command's own aesthetic (the
    /// non-pipe path); with `render == false` only the value is built (the
    /// pipe path renders the final result once).
    fn execute_single(&mut self, input: &str, render: bool) -> SmartResult<Value> {
        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let cmd = parts[0];
        let args = parts.get(1).unwrap_or(&"");

        match cmd {
            "network" => self.exec_network(args, render),
            #[cfg(feature = "domain-clipboard")]
            "clipboard" => self.exec_clipboard(args, render),
            #[cfg(feature = "domain-service")]
            "service" => self.exec_service(args, render),
            #[cfg(feature = "domain-files")]
            "files" => self.exec_files(args, render),
            #[cfg(feature = "domain-process")]
            "process" => self.exec_process(args, render),
            #[cfg(feature = "domain-settings")]
            "settings" => self.exec_settings(args, render),
            #[cfg(feature = "domain-logging")]
            "logging" => self.exec_logging(args, render),
            #[cfg(feature = "domain-clock")]
            "clock" => self.exec_clock(args, render),
            #[cfg(feature = "domain-time")]
            "time" => self.exec_time(args, render),
            #[cfg(feature = "domain-audio")]
            "audio" => self.exec_audio(args, render),
            #[cfg(feature = "domain-media")]
            "media" => self.exec_media(args, render),
            #[cfg(feature = "domain-power")]
            "power" => self.exec_power(args, render),
            #[cfg(feature = "domain-database")]
            "database" => self.exec_database(args, render),
            #[cfg(feature = "domain-queue")]
            "queue" => self.exec_queue(args, render),
            #[cfg(feature = "domain-appindex")]
            "appindex" => self.exec_appindex(args, render),
            #[cfg(feature = "domain-manifest")]
            "manifest" => self.exec_manifest(args, render),
            #[cfg(feature = "domain-vcard")]
            "vcard" => self.exec_vcard(args, render),
            #[cfg(feature = "domain-content")]
            "content" => self.exec_content(args, render),
            "migrate" => {
                if render {
                    print!("{}", migrate::render_queue(&OutputFormat::Human));
                    return Ok(Value::Null);
                }
                Ok(migrate::queue_value())
            }
            _ => {
                if migrate::find(cmd).is_some() {
                    // A domain the queue knows but the engine cannot serve
                    // yet: the honest answer, never a fake command.
                    if render {
                        println!("  {} {}", "◌".dimmed(), migrate::pending_reason(cmd));
                    }
                    Ok(Value::Null)
                } else {
                    Err(SmartError::failed(format!(
                        "Unknown command: {cmd}. Type 'help' for available commands."
                    )))
                }
            }
        }
    }

    /// The one real domain: parse its command with the same clap grammar the
    /// one-shot binary uses and route it to the engine, in-process.
    #[cfg(feature = "domain-network")]
    fn exec_network(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::network::{self, NetworkCmd};

        let mut argv: Vec<String> = vec!["network".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = NetworkCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = network::WriteGuard {
            allowed: self.accept_write(network::write_intent(&cmd), "")?,
        };
        if render {
            // A command that renders itself returns no further value.
            network::run(&cmd, &OutputFormat::Human, self.backend.as_ref(), guard)?;
            Ok(Value::Null)
        } else {
            network::evaluate(&cmd, self.backend.as_ref(), guard)
        }
    }

    #[cfg(not(feature = "domain-network"))]
    fn exec_network(&mut self, _args: &str, render: bool) -> SmartResult<Value> {
        if render {
            println!(
                "  {} network — unavailable: the shell was built without the domain-network \
                 feature, so the engine's network domain is not compiled",
                "✗".red()
            );
        }
        Ok(Value::Null)
    }

    /// The clipboard: parse its command with the same clap grammar the one-shot
    /// binary uses and route it to the native selection engine, in-process.
    /// Host-mutating verbs gate through the confirmation path (the REPL's
    /// prompt, a piped `y` line, or `--force`).
    #[cfg(feature = "domain-clipboard")]
    fn exec_clipboard(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::clipboard::{self, ClipboardCmd};

        let mut argv: Vec<String> = vec!["clipboard".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = ClipboardCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = clipboard::WriteGuard {
            allowed: self.accept_write(clipboard::write_intent(&cmd), "")?,
        };
        if render {
            clipboard::run(&cmd, &OutputFormat::Human, self.clipboard.as_ref(), guard)?;
            Ok(Value::Null)
        } else {
            clipboard::evaluate(&cmd, self.clipboard.as_ref(), guard)
        }
    }

    /// The service domain: parse its command with the same clap grammar the
    /// one-shot binary uses and route it to the supervisor's native read path,
    /// in-process. Host-mutating verbs gate through the confirmation path.
    #[cfg(feature = "domain-service")]
    fn exec_service(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::service::{self, ServiceCmd};
        // `Parser` in scope for the same clap grammar the one-shot binary uses
        // (a feature-only build has no other module importing it).
        use clap::Parser;

        let mut argv: Vec<String> = vec!["service".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = ServiceCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = service::WriteGuard {
            allowed: self.accept_write(service::write_intent(&cmd), "")?,
        };
        if render {
            service::run(&cmd, &OutputFormat::Human, self.service.as_ref(), guard)?;
            Ok(Value::Null)
        } else {
            service::evaluate(&cmd, self.service.as_ref(), guard)
        }
    }

    /// The files domain: parse with the same clap grammar the one-shot binary
    /// uses and route to the SESSION's backend, in-process. Host-mutating
    /// verbs gate through the confirmation path (the REPL's prompt, a piped
    /// `y` line, or `--force`).
    #[cfg(feature = "domain-files")]
    fn exec_files(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::files::{self, FilesCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["files".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = FilesCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        // The confirmation half: the context is a live read of the target(s)
        // (type/size/mtime, or old→new), so the prompt shows what the write
        // would touch — law #8's reads, never the write itself.
        let context = files::write_context(&cmd, self.files());
        let guard = files::WriteGuard {
            allowed: self.accept_write(files::write_intent(&cmd), &context)?,
        };
        // The watch ledger is this session's, as the clock's is, but the
        // borrow checker sees two &mut selves: the backend moves out for the
        // call and goes straight back.
        let files = self.files_backend.take().expect("the session's files backend");
        let result = if render {
            // The command rendered itself, so the REPL prints nothing further
            // (a rendered command answers with Null, never the value twice).
            files::run_session(
                &cmd,
                &OutputFormat::Human,
                files.as_ref(),
                guard,
                &mut self.session.files_watch,
            )
            .map(|_| Value::Null)
        } else {
            files::evaluate_session(&cmd, files.as_ref(), guard, &mut self.session.files_watch)
        };
        self.files_backend = Some(files);
        result
    }

    /// The process domain.
    #[cfg(feature = "domain-process")]
    fn exec_process(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::process::{self, ProcessCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["process".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = ProcessCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = process::WriteGuard {
            allowed: self.accept_write(process::write_intent(&cmd), "")?,
        };
        if render {
            process::run(&cmd, &OutputFormat::Human, self.process(), guard)?;
            Ok(Value::Null)
        } else {
            process::evaluate(&cmd, self.process(), guard)
        }
    }

    /// The settings domain.
    #[cfg(feature = "domain-settings")]
    fn exec_settings(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::settings::{self, SettingsCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["settings".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = SettingsCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let context = settings::write_context(&cmd, self.settings());
        let guard = settings::WriteGuard {
            allowed: self.accept_write(settings::write_intent(&cmd), &context)?,
        };
        if render {
            settings::run(&cmd, &OutputFormat::Human, self.settings(), guard)?;
            Ok(Value::Null)
        } else {
            settings::evaluate(&cmd, self.settings(), guard)
        }
    }

    /// The logging domain (all reads — no host-mutating verb, so no guard).
    #[cfg(feature = "domain-logging")]
    fn exec_logging(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::logging::{self, LoggingCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["logging".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = LoggingCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = logging::WriteGuard {
            allowed: self.accept_write(logging::write_intent(&cmd), "")?,
        };
        if render {
            logging::run(&cmd, &OutputFormat::Human, self.logging(), guard)?;
            Ok(Value::Null)
        } else {
            logging::evaluate(&cmd, self.logging(), guard)
        }
    }

    /// The clock domain: reads plus this session's timerfds and alarm store,
    /// all live for the process. The timer surface (create/list/check/cancel)
    /// drives the session's ledger, so the verbs compose across lines.
    #[cfg(feature = "domain-clock")]
    fn exec_clock(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::clock::{self, ClockCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["clock".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = ClockCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = clock::WriteGuard {
            allowed: self.accept_write(clock::write_intent(&cmd), "")?,
        };
        // The clock backend and its session ledger are both this shell's, but
        // the borrow checker sees two &mut selves: the backend is built (which
        // is also where the read is counted) and then moved out for the call
        // and put back, so the session and the engine are never aliased.
        let _ = self.clock();
        let clock = self.clock_backend.take().expect("the session's clock backend");
        let result = if render {
            // Rendered itself: the REPL prints nothing further.
            clock::run_session(
                &cmd,
                &OutputFormat::Human,
                clock.as_ref(),
                guard,
                &mut self.session.clock,
            )
            .map(|_| Value::Null)
        } else {
            clock::evaluate_session(&cmd, clock.as_ref(), guard, &mut self.session.clock)
        };
        self.clock_backend = Some(clock);
        result
    }

    /// The time domain (all reads of the kernel clock and its calendars).
    #[cfg(feature = "domain-time")]
    fn exec_time(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::time::{self, TimeCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["time".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = TimeCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        if render {
            time::run(&cmd, &OutputFormat::Human, self.time())?;
            Ok(Value::Null)
        } else {
            time::evaluate(&cmd, self.time())
        }
    }

    #[cfg(feature = "domain-audio")]
    fn exec_audio(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::audio::{self, AudioCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["audio".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = AudioCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = audio::WriteGuard { allowed: self.accept_write(audio::write_intent(&cmd), "")? };
        if render {
            audio::run(&cmd, &OutputFormat::Human, self.audio(), guard)?;
            Ok(Value::Null)
        } else {
            audio::evaluate(&cmd, self.audio(), guard)
        }
    }

    #[cfg(feature = "domain-media")]
    fn exec_media(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::media::{self, MediaCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["media".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = MediaCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = media::WriteGuard { allowed: self.accept_write(media::write_intent(&cmd), "")? };
        if render {
            media::run(&cmd, &OutputFormat::Human, self.media(), guard)?;
            Ok(Value::Null)
        } else {
            media::evaluate(&cmd, self.media(), guard)
        }
    }

    #[cfg(feature = "domain-power")]
    fn exec_power(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::power::{self, PowerCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["power".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = PowerCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = power::WriteGuard { allowed: self.accept_write(power::write_intent(&cmd), "")? };
        if render {
            power::run(&cmd, &OutputFormat::Human, self.power(), guard)?;
            Ok(Value::Null)
        } else {
            power::evaluate(&cmd, self.power(), guard)
        }
    }

    /// The database domain: the engine's own namespaced key/value store.
    #[cfg(feature = "domain-database")]
    fn exec_database(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::database::{self, DatabaseCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["database".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = DatabaseCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = database::WriteGuard {
            allowed: self.accept_write(database::write_intent(&cmd), "")?,
        };
        if render {
            database::run(&cmd, &OutputFormat::Human, self.database(), guard)?;
            Ok(Value::Null)
        } else {
            database::evaluate(&cmd, self.database(), guard)
        }
    }

    /// The queue domain: the engine's own FIFO store.
    #[cfg(feature = "domain-queue")]
    fn exec_queue(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::queue::{self, QueueCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["queue".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = QueueCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = queue::WriteGuard {
            allowed: self.accept_write(queue::write_intent(&cmd), "")?,
        };
        if render {
            queue::run(&cmd, &OutputFormat::Human, self.queue(), guard)?;
            Ok(Value::Null)
        } else {
            queue::evaluate(&cmd, self.queue(), guard)
        }
    }

    /// The appindex domain: the host's own desktop-entry corpus.
    #[cfg(feature = "domain-appindex")]
    fn exec_appindex(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::appindex::{self, AppindexCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["appindex".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = AppindexCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = appindex::WriteGuard {
            allowed: self.accept_write(appindex::write_intent(&cmd), "")?,
        };
        if render {
            appindex::run(&cmd, &OutputFormat::Human, self.appindex(), guard)?;
            Ok(Value::Null)
        } else {
            appindex::evaluate(&cmd, self.appindex(), guard)
        }
    }

    /// The manifest domain: the engine's own manifest store.
    #[cfg(feature = "domain-manifest")]
    fn exec_manifest(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::manifest::{self, ManifestCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["manifest".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = ManifestCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = manifest::WriteGuard {
            allowed: self.accept_write(manifest::write_intent(&cmd), "")?,
        };
        if render {
            manifest::run(&cmd, &OutputFormat::Human, self.manifest(), guard)?;
            Ok(Value::Null)
        } else {
            manifest::evaluate(&cmd, self.manifest(), guard)
        }
    }

    /// The vcard domain: the engine's own RFC 6350 parser and formatter.
    #[cfg(feature = "domain-vcard")]
    fn exec_vcard(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::vcard::{self, VcardCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["vcard".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = VcardCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = vcard::WriteGuard {
            allowed: self.accept_write(vcard::write_intent(&cmd), "")?,
        };
        if render {
            vcard::run(&cmd, &OutputFormat::Human, self.vcard(), guard)?;
            Ok(Value::Null)
        } else {
            vcard::evaluate(&cmd, self.vcard(), guard)
        }
    }

    /// The content domain: the engine's own content store.
    #[cfg(feature = "domain-content")]
    fn exec_content(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::content::{self, ContentCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["content".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = ContentCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = content::WriteGuard {
            allowed: self.accept_write(content::write_intent(&cmd), "")?,
        };
        if render {
            content::run(&cmd, &OutputFormat::Human, self.content(), guard)?;
            Ok(Value::Null)
        } else {
            content::evaluate(&cmd, self.content(), guard)
        }
    }

    // ── the write gate: the founder's standard ──────────────────────────────

    /// Whether this command performs a host write, and who accepted it.
    ///
    /// The founder's standard: a destructive verb asks `delete <path>? (y/N)`
    /// with the CONTEXT beside it (the live read's type/size/mtime, or the
    /// old→new of a move) — never "type the name". Script mode never assumes
    /// yes: with no piped answer line the verb REFUSES (`not_supported`, which
    /// the binary renders as a JSON answer under `--json`), and an explicit
    /// `--force` is the documented acceptance that skips the prompt. A read
    /// verb carries no intent and no prompt at all.
    fn accept_write(
        &self,
        intent: Option<(String, String)>,
        context: &str,
    ) -> SmartResult<bool> {
        accept_write(intent, context, self.gate)
    }

    // ── watch: the store's real bounded events ──────────────────────────────

    /// `watch network [--count N] [--interval MILLIS]` — the engine store's
    /// real event stream, bounded by the caller. One subscription per
    /// interface, one pull per round; the store revision is the event, and
    /// the revisions a pull collapsed are counted by the engine and reported.
    #[cfg(feature = "domain-network")]
    fn execute_watch(&mut self, domain: &str, args: &[String]) -> SmartResult<Value> {
        // `watch files <path>` is the engine's own inotify subscription: the
        // kernel's records, pulled in bounded rounds. Decided before the
        // network arm so a files-only build still serves it.
        #[cfg(feature = "domain-files")]
        if domain == "files" {
            return self.execute_watch_files(args);
        }

        let domain = domain.trim();
        if domain != "network" {
            println!(
                "  {} watch {} — {}",
                "◌".dimmed(),
                domain,
                migrate::pending_reason(domain)
            );
            return Ok(Value::Null);
        }

        let mut count: Option<u64> = None;
        let mut interval_ms: u64 = 1000;
        let mut rest = args.iter().map(String::as_str);
        while let Some(arg) = rest.next() {
            match arg {
                "--count" => {
                    let raw = rest.next();
                    count = Some(
                        raw.and_then(|v| v.parse::<u64>().ok()).ok_or_else(|| {
                            SmartError::invalid_argument("watch --count needs a number")
                        })?,
                    );
                }
                "--interval" => {
                    interval_ms = rest
                        .next()
                        .and_then(|v| v.parse::<u64>().ok())
                        .ok_or_else(|| {
                            SmartError::invalid_argument(
                                "watch --interval needs a number of milliseconds",
                            )
                        })?;
                }
                other => {
                    return Err(SmartError::invalid_argument(format!(
                        "watch: unknown option {other} (try --count N or --interval MILLIS)"
                    )))
                }
            }
        }

        let backend = self.backend.as_ref();
        let interfaces = backend
            .get_interfaces(smart_lib::domains::network::types::Pagination {
                offset: 0,
                limit: u32::MAX,
            })?
            .items;

        let mut subs = Vec::new();
        for iface in &interfaces {
            subs.push((iface.clone(), backend.subscribe_status(iface.id)?));
        }

        println!(
            "  {} watching network — {} interface(s), {} (Ctrl+C to stop)",
            "⟳".cyan(),
            subs.len(),
            match count {
                Some(n) => format!("{n} event(s) bounded"),
                None => "unbounded".to_string(),
            }
        );

        let mut pulled = 0u64;
        let mut round = 0u64;
        loop {
            if let Some(bound) = count {
                if round >= bound {
                    break;
                }
            }
            round += 1;
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            println!("  {} event {round} ({} so far)", "⟳".cyan(), pulled);
            for (iface, sub) in &subs {
                let status = backend.pull_status(*sub)?;
                println!(
                    "     ├─ {:<16} {:<9} {} hostState {} ({})",
                    iface.name,
                    status.kind,
                    if status.connected { "◉" } else { "○" },
                    status.host_state.as_token(),
                    status.host_state_source.as_token()
                );
                pulled += 1;
            }
        }

        for (_, sub) in subs {
            let _ = backend.cancel_subscription(sub);
        }
        Ok(Value::Null)
    }

    #[cfg(not(feature = "domain-network"))]
    fn execute_watch(&mut self, _domain: &str, _args: &[String]) -> SmartResult<Value> {
        Err(SmartError::unavailable(
            "watch needs the engine's network domain: rebuild the shell with default features",
        ))
    }

    /// `watch files <path> [--count N] [--interval MILLIS]` — the engine's own
    /// inotify subscription: one watch over the path, one bounded pull per
    /// round, the kernel's own records printed as they arrive. Bounded by the
    /// caller's count (Ctrl+C stops an unbounded stream), never polled.
    #[cfg(feature = "domain-files")]
    fn execute_watch_files(&mut self, args: &[String]) -> SmartResult<Value> {
        use smart_lib::domains::files::FilesBackend;

        let mut path: Option<String> = None;
        let mut count: Option<u64> = None;
        let mut interval_ms: u64 = 500;
        let mut rest = args.iter().map(String::as_str);
        while let Some(arg) = rest.next() {
            match arg {
                "--count" => {
                    count = Some(
                        rest.next()
                            .and_then(|v| v.parse::<u64>().ok())
                            .ok_or_else(|| {
                                SmartError::invalid_argument("watch --count needs a number")
                            })?,
                    );
                }
                "--interval" => {
                    interval_ms = rest
                        .next()
                        .and_then(|v| v.parse::<u64>().ok())
                        .ok_or_else(|| {
                            SmartError::invalid_argument(
                                "watch --interval needs a number of milliseconds",
                            )
                        })?;
                }
                other if other.starts_with("--") => {
                    return Err(SmartError::invalid_argument(format!(
                        "watch files: unknown option {other} (try --count N or --interval MILLIS)"
                    )))
                }
                other => path = Some(other.to_string()),
            }
        }
        let Some(path) = path else {
            return Err(SmartError::invalid_argument(
                "watch files names a directory: `watch files /tmp`",
            ));
        };

        let backend = smart_lib::platforms::linux::files::LinuxFilesBackend::new();
        let subscription = backend.watch_directory(&path)?;
        println!(
            "  {} watching {} — {} the kernel's own inotify records (Ctrl+C to stop)",
            "⟳".cyan(),
            path.clone().bold(),
            match count {
                Some(n) => format!("{n} round(s) bounded"),
                None => "unbounded".to_string(),
            }
        );
        let mut seen = 0u64;
        let mut round = 0u64;
        loop {
            if let Some(bound) = count {
                if round >= bound {
                    break;
                }
            }
            round += 1;
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            let events = backend.pull_watch(subscription, 32)?;
            for event in &events {
                seen += 1;
                let glyph = match event.kind {
                    smart_lib::domains::files::types::FileEventKind::Create => "＋",
                    smart_lib::domains::files::types::FileEventKind::Delete => "－",
                    smart_lib::domains::files::types::FileEventKind::Modify => "～",
                    smart_lib::domains::files::types::FileEventKind::Rename
                    | smart_lib::domains::files::types::FileEventKind::Move => "↔",
                    _ => "▢",
                };
                let shown = if event.old_path.is_empty() {
                    event.path.clone()
                } else {
                    format!("{} → {}", event.old_path, event.path)
                };
                println!(
                    "     {} {:<7} {} {}",
                    glyph,
                    event.kind.as_token(),
                    shown,
                    format!("(event {seen})").dimmed()
                );
            }
        }
        let _ = backend.cancel_watch(subscription);
        println!(
            "  {} watch closed — {} event(s) over {} round(s)",
            "◉".green(),
            seen,
            round
        );
        Ok(Value::Null)
    }

    // ── apply / export: the real [network] section ──────────────────────────

    /// `apply <file.toml>` — read the config, reconcile each `[network]`
    /// entry against the engine's live read surface, and issue the engine
    /// call only where the desired state differs. An entry already satisfied
    /// is a read, not a write — so applying the host's own observed state
    /// changes nothing.
    #[cfg(feature = "domain-network")]
    fn execute_apply(&mut self, path: &str) -> SmartResult<Value> {
        let content = std::fs::read_to_string(path)?;
        let config: toml::Value = toml::from_str(&content)?;
        let Some(network) = config.get("network") else {
            return Err(SmartError::invalid_argument(format!(
                "{path}: no [network] section (the only section this slice's engine serves)"
            )));
        };

        let backend = self.backend.as_ref();
        println!("  {} Applying {} [network]...", "⚙".cyan(), path);

        let mut applied = 0u32;
        if let Some(desired) = network.get("networking").and_then(|v| v.as_bool()) {
            let current = backend.is_networking_enabled()?;
            if current == desired {
                println!(
                    "  ├─ networking  {} {} (engine read — satisfied)",
                    "◉".green(),
                    if desired { "on" } else { "off" }
                );
            } else {
                let allowed = confirm_host_write("networking", &format!("networking → {}", if desired { "on" } else { "off" }));
                backend.set_networking_enabled(desired)?;
                applied += 1;
                println!(
                    "  ├─ networking  {} → {} {}",
                    "+".green(),
                    if desired { "on" } else { "off" },
                    if allowed { "(confirmed)" } else { "(forced by config)" }
                );
            }
        }

        if let Some(radios) = network.get("radios").and_then(|v| v.as_array()) {
            for entry in radios {
                let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("wifi");
                let desired = entry.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                let page = backend.list_radio_states(
                    smart_lib::domains::network::types::Pagination { offset: 0, limit: u32::MAX },
                )?;
                let current = page.items.iter().find(|r| r.name == name).map(|r| r.enabled);
                if current == Some(desired) {
                    println!(
                        "  ├─ radio.{:<6} {} {} (engine read — satisfied)",
                        name,
                        "◉".green(),
                        if desired { "on" } else { "off" }
                    );
                } else {
                    confirm_host_write("radio", &format!("{name} → {}", if desired { "on" } else { "off" }));
                    backend.set_radio_enabled(name, desired)?;
                    applied += 1;
                    println!(
                        "  ├─ radio.{:<6} {} → {}",
                        name,
                        "+".green(),
                        if desired { "on" } else { "off" }
                    );
                }
            }
        }

        if applied == 0 {
            println!("  └─ {} nothing to write — the engine reads satisfy every entry", "✓".green());
        } else {
            println!("  └─ {} {applied} engine write(s) issued", "✓".green());
        }
        Ok(Value::Null)
    }

    #[cfg(not(feature = "domain-network"))]
    fn execute_apply(&mut self, path: &str) -> SmartResult<Value> {
        let _ = path;
        Err(SmartError::unavailable(
            "apply's [network] section needs the engine's network domain",
        ))
    }

    /// `export [path]` — the host's observed network state as TOML, from the
    /// engine's reads. The file written is a real `[network]` section, the
    /// same shape `apply` reconciles.
    #[cfg(feature = "domain-network")]
    fn execute_export(&mut self, path: Option<&str>) -> SmartResult<Value> {
        let backend = self.backend.as_ref();
        let all = smart_lib::domains::network::types::Pagination { offset: 0, limit: u32::MAX };
        let networking = backend.is_networking_enabled()?;
        let radios = backend.list_radio_states(all)?.items;

        let mut toml = String::new();
        toml.push_str("# smart export — the engine's observed network state\n");
        toml.push_str("[network]\n");
        toml.push_str(&format!("networking = {networking}\n"));
        for radio in &radios {
            toml.push_str("\n[[network.radios]]\n");
            toml.push_str(&format!("name = \"{}\"\n", radio.name));
            toml.push_str(&format!("enabled = {}\n", radio.enabled));
            toml.push_str(&format!("hard_blocked = {}\n", radio.hard_blocked));
            toml.push_str(&format!("soft_blocked = {}\n", radio.soft_blocked));
        }

        match path {
            Some(p) => {
                std::fs::write(p, &toml)?;
                println!("  {} exported network state → {}", "✓".green(), p);
            }
            None => println!("{toml}"),
        }
        Ok(Value::Null)
    }

    #[cfg(not(feature = "domain-network"))]
    fn execute_export(&mut self, _path: Option<&str>) -> SmartResult<Value> {
        Err(SmartError::unavailable(
            "export's [network] section needs the engine's network domain",
        ))
    }

    // ── the session's own read counts ───────────────────────────────────────

    /// `session reads` — what the session actually read, per domain: how many
    /// commands evaluated the domain and how many backends it built. One build
    /// per domain per process is the measurement that the per-invocation-backend
    /// rot is gone; a pipe (`process | sort mem | take 5`) reads once and pipes
    /// the VALUE through every stage, so its read count is 1.
    fn execute_session(&self, action: &str) -> SmartResult<Value> {
        match action.trim() {
            "reads" | "" => {
                let reads = self.session.reads();
                let mut out = String::new();
                out.push_str(&format!(
                    "  {} {} {} — one backend per domain per session; pipes read once and pipe the VALUE\n",
                    "📈".blue().bold(),
                    "Session reads".blue().bold(),
                    format!("({})", reads.len()).dimmed()
                ));
                if reads.is_empty() {
                    out.push_str(&format!(
                        "  └─ {} no domain read yet this session\n",
                        "○".dimmed()
                    ));
                    return Ok(Value::Null);
                }
                let mut table = Table::new(vec![
                    "DOMAIN".into(),
                    "READS".into(),
                    "BACKENDS BUILT".into(),
                ]);
                for row in &reads {
                    table = table.row(vec![
                        Cell::plain(row.domain),
                        Cell::plain(row.reads.to_string()),
                        Cell::styled(row.builds.to_string(), if row.builds > 1 { Sgr::Yellow } else { Sgr::Plain }),
                    ]);
                }
                out.push_str(&table.render());
                print!("{out}");
                Ok(Value::Null)
            }
            other => Err(SmartError::invalid_argument(format!(
                "session: unknown surface {other} (the session answers `session reads`)"
            ))),
        }
    }

    // ── help: only what is real ─────────────────────────────────────────────

    /// `help` (no domain) — the domains that are LIVE today, in the board's own
    /// order. The list is the BOARD's, never a hand-kept one: a domain appears
    /// because `migrate` records it `RealToday` on the engine, which is what
    /// stopped the old `help` listing 3 of 9 live domains and calling the other
    /// six "the migration queue" while all nine answered commands.
    fn help_overview(&self) {
        let live = crate::migrate::live_domains();
        let (_real, pending, archive) = crate::migrate::counts();
        println!(
            "  {} Smart Shell — commands: {} domains LIVE on the engine",
            "●".blue().bold(),
            live.len()
        );
        println!();
        for name in &live {
            let gloss = LIVE_DOMAIN_GLOSS
                .iter()
                .find(|(d, _)| d == name)
                .map(|(_, g)| *g)
                .unwrap_or("live on the engine — see `smart migrate`");
            println!("    {:<24} {}", name, gloss);
        }
        println!("    {:<24} {}", "session", "this session's own read counts (`session reads`)");
        println!("    {:<24} {}", "migrate", "the migration queue as data");
        println!();
        println!("  {} Special:", "●".blue().bold());
        println!("    @<domain>/<field>     Selector (e.g. @process/processes)");
        println!("    <cmd> | where ...     Pipe with filter (the ops reshape a VALUE)");
        println!("    <cmd> | sort <field>  Sort by field");
        println!("    <cmd> | take <n>      Take the first n items");
        println!("    <cmd> | skip <n>      Drop the first n items");
        println!("    <cmd> | count         Count items");
        println!("    watch <domain>        Stream the store's real events");
        println!("    apply <file.toml>     Apply a config ([network] section)");
        println!("    export [path]         Export the observed state");
        println!("    session reads         What this session read, per domain");
        println!("    help [domain]         This list, or one domain's verbs");
        println!("    exit                  Leave");
        println!();
        println!(
            "  {} Language (one command per line): a variable assigns, a pipe reshapes, and",
            "●".blue().bold()
        );
        println!("    `-c '<line>'` runs one line the same way the REPL does. An `if:`/`for:`/");
        println!("    `fn` block is entered as a single `-c` line — the REPL takes one line per");
        println!("    prompt and never guesses at an indented body.");
        println!();
        println!(
            "  {} {} queued domain(s) and {} archive-only domain(s) are NOT live here —",
            "◌".dimmed(),
            pending,
            archive
        );
        println!(
            "    each answers honestly from the board (`smart migrate` lists every one; a",
        );
        println!("    queued domain answers with its migration position, never a fake command).");
    }

    fn execute_help(&self, domain: Option<&str>) {
        match domain {
            Some("network") => {
                println!("  {} network — Commands:", "?".blue());
                println!("    network                       The interface table (default view)");
                println!("    network status                Networking, radios, interfaces with hostState");
                println!("    network interfaces            Every interface the kernel reports");
                println!("    network radios                Radio states (hard/soft blocks separate)");
                println!("    network interface <name>      One interface: type + hostState");
                println!("    network wifi                  The wifi radio's state");
                println!("    network wifi list             Remembered networks");
                println!("    network wifi scan             An active scan (the engine decides)");
                println!("    network wifi connect <ssid>   Connect [password]");
                println!("    network wifi disconnect       Disconnect");
                println!("    network wifi forget <ssid>    Forget a remembered network");
                println!("    network wifi on|off           The radio switch (a real engine call)");
                println!();
                println!("  Selectors:");
                println!("    @network/interfaces           The interface records");
                println!("    @network/radios               The radio records");
                println!("    @network/known                The remembered networks");
                println!("    @network/status               Per-interface status records");
                println!("    @network/networking           The master switch");
            }
            #[cfg(feature = "domain-clipboard")]
            Some("clipboard") => {
                println!("  {} clipboard — Commands:", "?".blue());
                println!("    clipboard                     The current selection (transport + content)");
                println!("    clipboard get [--mime T]      Read the selection");
                println!("    clipboard set <text>          Write it (host-mutating, gate-routed)");
                println!("    clipboard clear               Empty the selection (host-mutating)");
                println!("    clipboard history [sub]       The engine-side ledger of this process's copies");
                println!("    clipboard types|limits|preview|validate|convert");
                println!();
                println!("  Selectors:");
                println!("    @clipboard/content            The current selection");
                println!("    @clipboard/transport          The native transport in use");
                println!("    @clipboard/mimes              The targets the owner offers");
            }
            #[cfg(feature = "domain-service")]
            Some("service") => {
                println!("  {} service — Commands:", "?".blue());
                println!("    service                       Every service the supervisor serves");
                println!("    service status [name]         The table, or one service's state");
                println!("    service inspect <name>        One service's state + health");
                println!("    service health [--quarantined] The health listing");
                println!("    service deps <name>           The dependencies it declares");
                println!("    service resolve <name>        A name to an identity");
                println!("    service watch [name] [-c N]   Bounded state-change events");
                println!("    service controls [verb]       The one gated surface's honest status");
                println!("    service metrics|allocation|check <name>");
                println!("                                   What a supervisor does not own (honest)");
                println!("    service logs <name>           The log surface (logging.capnp's)");
                println!("    service start|stop|restart <name>  Host-mutating (gate-routed)");
                println!();
                println!("  Selectors:");
                println!("    @service/services             The service records");
                println!("    @service/supervisor           The supervisor's identity + caps");
            }
            #[cfg(feature = "domain-files")]
            Some("files") => {
                println!("  {} files — Commands:", "?".blue());
                println!("    files [path]                  The listing (getdents64; --all/--pattern/");
                println!("                                   --sort/--order/--recursive)");
                println!("    files find <path> <glob>      A name search under a path");
                println!("    files tree <path> [--depth N] A bounded walk");
                println!("    files stat|xattrs <path>      statx metadata (+ the file's own xattrs)");
                println!("    files cat <path> [--offset --bytes]  Read through the handle surface");
                println!("    files du|mounts [path]        statvfs usage; the mount this path lives on");
                println!("    files locks|hash <path>       This process's fcntl locks; SHA-256");
                println!("    files versions|branches <path>|store|dedupe   The version store");
                println!("    files watch <path> [-c N]     Real inotify events, bounded");
                println!("    files watches                 The watch ledger: whose subscriptions");
                println!("                                   the host's /proc can still see");
                println!("    files special <token>         home/tmp/cwd/... resolved from the engine");
                println!("    files trash|recents           The trash and the recently-used record");
                println!("    files app <mime>              The handler for a MIME type");
                println!("    files grep <path> <pattern>   A pattern over a file's lines");
                println!("    files compare <a> <b>|health|quota|caps");
                println!("    files mkdir|touch|rm|mv|cp|write|link|lock|unlock   Host-mutating");
                println!("                                   (every one prompts `delete <path>? (y/N)`");
                println!("                                   with the type/size/mtime beside it)");
                println!();
                println!("  Selectors:");
                println!("    @files/entries|home|cwd|du|caps");
                println!("    files ls | take 5             Every row the read found — `| take` narrows");
            }
            #[cfg(feature = "domain-process")]
            Some("process") => {
                println!("  {} process — Commands:", "?".blue());
                println!("    process                       The table: every process /proc reports");
                println!("    process <pid>                 One process's whole read surface");
                println!("    process inspect|tree|threads|env|maps|limits|accounting <pid>");
                println!("                                   (env --names-only hides the values)");
                println!("    process groups|group <pgid>   The kernel's own job-control groups");
                println!("    process stats [--uid N]       The aggregate over every process");
                println!("    process ptrace|can-signal <pid>   The host's own measured verdicts");
                println!("    process signal <pid> <sig>    Deliver a signal (host-mutating)");
                println!("    process attach <pid>          A ptrace attach (host-mutating)");
                println!("    process wait <pid>|caps");
                println!();
                println!("  Selectors:");
                println!("    @process/processes            The process records (one field per");
                println!("                                   record: `@process/processes where uid == 1000`)");
                println!("    process | sort mem | take 5   The read happens ONCE; the pipe reshapes");
            }
            #[cfg(feature = "domain-settings")]
            Some("settings") => {
                println!("  {} settings — Commands:", "?".blue());
                println!("    settings                      Every schema the host publishes (default)");
                println!("    settings list <domain>        One schema's live keys and values");
                println!("    settings get <domain> <key>   One key: value, range, summary");
                println!("    settings schema <domain>      The declaration per key");
                println!("    settings category <token>     The keys recorded under one category");
                println!("    settings version|history|diff The store's own bookkeeping");
                println!("    settings bulk-get <domain> k… Several keys in one read");
                println!("    settings watch [-c N]         Bounded change events");
                println!("    settings set <domain> <key> <value>   Host-mutating (prompts)");
                println!("    settings bulk-set <domain> a=… b=…     Host-mutating (prompts)");
                println!("    settings caps");
                println!();
                println!("  Selectors:");
                println!("    @settings/domains|schema|caps");
            }
            #[cfg(feature = "domain-logging")]
            Some("logging") => {
                println!("  {} logging — Commands:", "?".blue());
                println!("    logging sinks [--refresh]     Every sink the host has, measured by");
                println!("                                   opening it (root-only ones answer the");
                println!("                                   host's own Permission denied words)");
                println!("    logging domains               Every domain: file/sink, level, sampling");
                println!("    logging tail <sink> [-n N]    Real lines out of a real host log");
                println!("    logging entries [--level --domain --pattern]  This process's store");
                println!("    logging follow [-c N]         A real subscription, bounded");
                println!("    logging emit <level> <msg>    Write a line to this process's stderr");
                println!("    logging level|set-level|sampling <domain>");
                println!("    logging clear <domain>|export|tracked|caps");
                println!();
                println!("  Selectors:");
                println!("    @logging/sinks|domains|caps");
            }
            #[cfg(feature = "domain-clock")]
            Some("clock") => {
                println!("  {} clock — Commands:", "?".blue());
                println!("    clock                         The clock info (the default view)");
                println!("    clock now|monotonic|uptime|resolution|sync|security|domains");
                println!("    clock quality <source>        One source's measured quality");
                println!("    clock drift <a> <b>           The drift between two clock domains");
                println!("    clock sleep <ms>              The engine's own sleep");
                println!("    clock timer create [--after-ms N --interval-ms N --label L]");
                println!("                                   Arms a real timerfd the session owns");
                println!("    clock timer list              This session's timers (the ledger)");
                println!("    clock timer check <id>        The kernel's own expiration count");
                println!("    clock timer cancel <id>       Disarm (host-mutating — prompts)");
                println!("    clock alarm list|next|create|snooze|delete   The alarm store");
                println!("    clock virtual create|advance <id> <ms>   Software clocks");
                println!("    clock sleep-cancellable <ms> [--cancel-after N]");
                println!("    clock watch [-c N]|caps");
                println!();
                println!("  Selectors:");
                println!("    @clock/info|sync|caps");
                println!("    clock timer create            Timers live for the WHOLE session: a fresh");
                println!("                                   process reads the ledger rows earlier ones");
                println!("                                   left (owned / gone), never a firing");
            }
            #[cfg(feature = "domain-time")]
            Some("time") => {
                println!("  {} time — Commands:", "?".blue());
                println!("    time                          Wall time, weekday, day-of-year, zone");
                println!("    time tai                      UTC nanos <-> TAI (offset, round-trip)");
                println!("    time cal [--date D] [--to Z]  Gregorian into the contract's calendars");
                println!("    time add|sub <date> <unit>    Calendar arithmetic (month-end clamped)");
                println!("    time diff <a> <b>             The calendar distance between two dates");
                println!("    time caps");
                println!();
                println!("  Selectors:");
                println!("    @time/* resolves through the calendar set (see `smart migrate`)");
            }
            Some("shell") | Some("session") => {
                println!("  {} session — Commands:", "?".blue());
                println!("    session reads                 Per domain: how many commands read it and");
                println!("                                   how many backends the session built (one");
                println!("                                   per domain per process — a pipe reads once");
                println!("                                   and pipes the VALUE through every stage)");
            }
            Some(other) => {
                println!(
                    "  {} {} — {}",
                    "◌".dimmed(),
                    other,
                    crate::migrate::pending_reason(other)
                );
            }
            None => self.help_overview(),
        }
    }

    // ── the parser ──────────────────────────────────────────────────────────

    /// Parse one line (or a multi-line block) into a command. Static: the
    /// grammar needs no engine, so a function body can be parsed before any
    /// backend exists.
    pub fn parse(input: &str) -> ShellCommand {
        let input = input.trim();
        if input.is_empty() {
            return ShellCommand::Empty;
        }

        if input == "exit" || input == "quit" {
            return ShellCommand::Exit;
        }

        if input == "migrate" || input == "migration" {
            return ShellCommand::Migrate;
        }

        if input.starts_with("help") {
            let domain = input
                .strip_prefix("help")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            return ShellCommand::Help { domain };
        }

        if input.starts_with("watch ") {
            let rest = input.strip_prefix("watch ").unwrap().trim().to_string();
            let mut words = rest.split_whitespace();
            let domain = words.next().unwrap_or("").to_string();
            let args: Vec<String> = words.map(str::to_string).collect();
            return ShellCommand::Watch { domain, args };
        }

        if input.starts_with("session") {
            let action = input
                .strip_prefix("session")
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            return ShellCommand::Session { action };
        }

        if input.starts_with("apply ") {
            let path = input.strip_prefix("apply ").unwrap().trim().to_string();
            return ShellCommand::Apply { path };
        }

        if input == "export" {
            return ShellCommand::Export { path: None };
        }

        if input.starts_with("export ") {
            let path = input.strip_prefix("export ").unwrap().trim().to_string();
            return ShellCommand::Export { path: Some(path) };
        }

        if input.starts_with("fn ") {
            let rest = input.strip_prefix("fn ").unwrap();
            let (name, params_body) = rest.split_once('(').unwrap_or((rest, ")"));
            let params_str = params_body
                .split_once(')')
                .map(|(p, _)| p)
                .unwrap_or(params_body);
            let params: Vec<String> = params_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let body_str = input
                .split_once('{')
                .map(|(_, b)| b.trim_end_matches('}').trim())
                .or_else(|| input.split_once(':').map(|(_, b)| b.trim()))
                .unwrap_or("");
            let body_lines = parse_body(body_str);
            return ShellCommand::Fn { name: name.trim().to_string(), params, body: body_lines };
        }

        if input.starts_with("for ") {
            let rest = input.strip_prefix("for ").unwrap();
            if let Some((var, rest2)) = rest.split_once(" in ") {
                // The one-line form carries its body after a colon; the block
                // form has its body on the following lines.
                let (iter_src, inline_body) = match rest2.split_once(':') {
                    Some((head, body)) => (head.trim(), body.trim()),
                    None => (rest2.trim().trim_end_matches(':').trim(), ""),
                };
                let iter = Shell::parse_expr(iter_src);
                let body_lines = if !inline_body.is_empty() {
                    parse_body(inline_body)
                } else {
                    parse_body(
                        input
                            .split_once('{')
                            .map(|(_, b)| b.trim_end_matches('}').trim())
                            .unwrap_or(""),
                    )
                };
                return ShellCommand::For { var: var.trim().to_string(), iter, body: body_lines };
            }
        }

        if input.starts_with("if ") {
            return parse_if_block(input);
        }

        if let Some(eq_pos) = input.find(" = ") {
            let name = input[..eq_pos].trim().to_string();
            let value = Shell::parse_expr(input[eq_pos + 3..].trim());
            return ShellCommand::Assign { name, value };
        }

        ShellCommand::Exec(input.to_string())
    }

    /// Parse an expression: a selector, a literal, a variable, a comparison,
    /// or a pipe of any of those — the pipe expression is live, so
    /// `var = @network/interfaces | count` evaluates through the same
    /// evaluator as `network | where …`.
    fn parse_expr(input: &str) -> Expr {
        let input = input.trim();

        // A selector expression: the grammar's own three shapes, in order —
        // a pipe of a selector, a selector with its `where` filter, or a
        // comparison of the selector's field. The comparison scan comes last
        // because a `where` filter legitimately contains `==`, which is the
        // selector's filter syntax, not the expression's comparison.
        if input.starts_with('@') {
            if let Some(bar) = input.find('|') {
                let head = Shell::parse_expr(&input[..bar]);
                let ops = pipe::parse_pipe_chain(&input[bar..]).unwrap_or_default();
                return Expr::Pipe { expr: Box::new(head), ops };
            }
            if !input.contains(" where ") {
                if let Some(comparison) = Shell::scan_comparison(input) {
                    return comparison;
                }
            }
            return Expr::Selector(input.to_string());
        }

        if input.contains('|') {
            if let Some(bar) = input.find('|') {
                let head = Shell::parse_expr(&input[..bar]);
                let ops = pipe::parse_pipe_chain(&input[bar..]).unwrap_or_default();
                return Expr::Pipe { expr: Box::new(head), ops };
            }
        }

        if let Ok(n) = input.parse::<f64>() {
            return Expr::Literal(Value::Number(n));
        }

        if input == "true" {
            return Expr::Literal(Value::Bool(true));
        }
        if input == "false" {
            return Expr::Literal(Value::Bool(false));
        }

        if input.starts_with('$') {
            return Expr::Variable(input[1..].to_string());
        }

        if input.starts_with('"') && input.ends_with('"') && input.len() >= 2 {
            return Expr::Literal(Value::String(input[1..input.len() - 1].to_string()));
        }

        if let Some(comparison) = Shell::scan_comparison(input) {
            return comparison;
        }

        Expr::Literal(Value::String(input.to_string()))
    }

    /// A comparison over an expression, or `None` when the text carries no
    /// comparison operator. Both sides are themselves full expressions, so
    /// `$n == 2` compares a variable and a literal.
    fn scan_comparison(input: &str) -> Option<Expr> {
        for op in &["==", "!=", ">=", "<=", ">", "<"] {
            if let Some(pos) = input.find(op) {
                let left = Shell::parse_expr(&input[..pos]);
                let right = Shell::parse_expr(&input[pos + op.len()..]);
                let comp_op = match *op {
                    "==" => CompOp::Eq,
                    "!=" => CompOp::Ne,
                    ">" => CompOp::Gt,
                    "<" => CompOp::Lt,
                    ">=" => CompOp::Gte,
                    "<=" => CompOp::Lte,
                    _ => unreachable!(),
                };
                return Some(Expr::Comparison {
                    left: Box::new(left),
                    op: comp_op,
                    right: Box::new(right),
                });
            }
        }
        None
    }
}

/// The founder's confirmation standard, as ONE function both doors call: the
/// REPL's line dispatcher and the one-shot binary's domain dispatchers.
///
/// A destructive verb asks `delete <path>? (y/N)` with the CONTEXT beside it
/// (the live read's type/size/mtime, or the old→new of a move) — never "type
/// the name". The three answers:
///
/// * `Ok(true)` — the operator said **`--force`** on the command line (the
///   documented acceptance), or answered the prompt with an explicit `y`/`yes`
///   on stdout's own question (an interactive terminal or a piped `y` line).
/// * `Err(Cancelled)` — the operator answered `n`, or pressed Enter: an empty
///   answer is a NO, and nothing was written.
/// * `Err(NotSupported)` — there was nobody to ask (a non-interactive caller
///   with no piped answer line, i.e. `--json`/piped/redirected script mode),
///   which the binary renders as a deterministic JSON error answer.
pub fn accept_write(
    intent: Option<(String, String)>,
    context: &str,
    gate: WriteGate,
) -> SmartResult<bool> {
    let Some((verb, target)) = intent else {
        return Ok(false);
    };
    // The question, built once here because every answer path names it.
    // The founder's question, verbatim: `delete <path>? (y/N)` with the CONTEXT
    // beside it. The target is always named — the path being destroyed is the
    // first thing an operator needs — and the context never repeats a target
    // it already carries.
    let target = target.trim();
    let question = match (target, context) {
        ("", "") => verb.clone(),
        (t, "") => format!("{verb} {t}"),
        ("", c) => format!("{verb} {c}"),
        (t, c) if c.contains(t) => format!("{verb} {c}"),
        (t, c) => format!("{verb} {t} ({c})"),
    };
    if gate.dry_run {
        // The plan IS the answer: what would be asked, with the live context
        // beside it, and an explicit statement that nothing was written.
        println!(
            "  {} (y/N) — the plan; nothing was written (--dry-run)",
            format!("{question}?").bold()
        );
        return Err(SmartError::not_supported(format!(
            "--dry-run: {question} — the plan was printed and no write was performed"
        )));
    }
    if gate.force {
        // The operator said yes on the command line; the prompt is skipped
        // and the refusal message says so.
        return Ok(true);
    }
    if gate.script_mode {
        // Stdin is the caller's program, never an answer: the verb says so
        // and refuses, which is what `--json` answers with under script mode.
        return Err(SmartError::not_supported(format!(
            "refusing `{question}`: it CHANGES state — the host's own, or the engine's own \
             store — and this caller is driving stdin itself (script mode), so there is no \
             operator to ask. Answer the interactive `(y/N)` prompt when running interactively, \
             pipe a `y` line to a one-shot invocation, or pass `--force` to accept the write \
             explicitly (the live bring-back proof is founder-run)."
        )));
    }
    match crate::confirm::ask(&question) {
        crate::confirm::Answer::Yes => Ok(true),
        crate::confirm::Answer::No => Err(SmartError::cancelled(format!(
            "{question} — the operator answered no; nothing was written"
        ))),
        crate::confirm::Answer::NoInput => Err(SmartError::not_supported(format!(
            "refusing `{question}`: it CHANGES state — the host's own, or the engine's own \
             store — and there is no operator to ask (non-interactive). Answer the interactive \
             `(y/N)` prompt when running interactively, pipe a `y` line, or pass `--force` to \
             accept the write explicitly (the live bring-back proof is founder-run)."
        ))),
    }
}

/// Parse a command body: one command per line, empty lines dropped.
fn parse_body(body: &str) -> Vec<ShellCommand> {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(Shell::parse)
        .collect()
}

/// Parse an `if` command with its `elif`/`else` branches — the branch the
/// original's parser never filled. Accepts both a block:
///
/// ```text
/// if <cond>:
///   <body>
/// elif <cond>:
///   <body>
/// else:
///   <body>
/// ```
///
/// and the one-line form `if <cond>: <body>`.
fn parse_if_block(input: &str) -> ShellCommand {
    let lines: Vec<&str> = input.lines().collect();
    let (first_cond, first_inline) = split_header(&lines[0], "if ");
    let mut command = ShellCommand::If {
        condition: Shell::parse_expr(first_cond.trim()),
        then_body: parse_body(first_inline),
        elifs: Vec::new(),
        else_body: Vec::new(),
    };

    enum Current {
        Then,
        Elif(usize),
        Else,
    }
    let mut current = Current::Then;

    for line in &lines[1..] {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((cond, inline)) = header_of(line, "elif ") {
            if let ShellCommand::If { elifs, .. } = &mut command {
                elifs.push((Shell::parse_expr(cond.trim()), parse_body(inline)));
            }
            current = Current::Elif(elifs_len(&command) - 1);
        } else if let Some((_, inline)) = header_of(line, "else:") {
            current = Current::Else;
            if let ShellCommand::If { else_body, .. } = &mut command {
                *else_body = parse_body(inline);
            }
        } else {
            let body = parse_body(line);
            match &mut command {
                ShellCommand::If { then_body, elifs, else_body, .. } => match current {
                    Current::Then => then_body.extend(body),
                    Current::Elif(i) => elifs[i].1.extend(body),
                    Current::Else => else_body.extend(body),
                },
                _ => {}
            }
        }
    }
    command
}

fn elifs_len(command: &ShellCommand) -> usize {
    match command {
        ShellCommand::If { elifs, .. } => elifs.len(),
        _ => 0,
    }
}

/// Split a header line (`if <cond>:` / `elif <cond>:`) into its condition and
/// its inline body (the text after the colon, if any).
fn split_header<'a>(line: &'a str, prefix: &str) -> (String, &'a str) {
    header_of(line, prefix).unwrap_or_else(|| (line.trim_start_matches(prefix).to_string(), ""))
}

/// A header is `<keyword> <text>:`; the returned inline body is the text
/// after the colon (empty when the body follows on later lines).
fn header_of<'a>(line: &'a str, prefix: &str) -> Option<(String, &'a str)> {
    let rest = line.strip_prefix(prefix)?;
    let (cond, inline) = match rest.split_once(':') {
        Some((c, i)) => (c, i),
        None => (rest, ""),
    };
    Some((cond.trim().to_string(), inline.trim()))
}

// ── the REPL ───────────────────────────────────────────────────────────────

/// The rustyline helper: the salvaged `Completer`, finally wired (the
/// original built it and never installed it).
#[cfg(feature = "domain-network")]
struct ShellHelper {
    completer: Completer,
}

#[cfg(feature = "domain-network")]
impl rustyline::completion::Completer for ShellHelper {
    type Candidate = rustyline::completion::Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<rustyline::completion::Pair>)> {
        let input = &line[..pos];
        let start = input
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);
        let pairs = self
            .completer
            .complete(input)
            .into_iter()
            .map(|c| rustyline::completion::Pair {
                display: c.clone(),
                replacement: c,
            })
            .collect();
        Ok((start, pairs))
    }
}

#[cfg(feature = "domain-network")]
impl rustyline::hint::Hinter for ShellHelper {
    type Hint = String;
}

#[cfg(feature = "domain-network")]
impl rustyline::highlight::Highlighter for ShellHelper {}

#[cfg(feature = "domain-network")]
impl rustyline::validate::Validator for ShellHelper {}

#[cfg(feature = "domain-network")]
impl rustyline::Helper for ShellHelper {}

/// Run the shell: the rustyline REPL on a terminal, the same evaluator over
/// stdin's lines when piped (so the language is scriptable and provable).
pub fn run_shell(backend: Box<Backend>) {
    run_shell_with(backend, WriteGate::default())
}

/// The binary's entry: the same REPL, with `--force` carried into the session
/// and the clock-timer ledger shared with the next process through the shell's
/// own state directory.
pub fn run_shell_with(backend: Box<Backend>, gate: WriteGate) {
    let mut shell = Shell::new(backend);
    shell.set_gate(gate);
    shell.enable_state_persistence();
    #[cfg(feature = "domain-clipboard")]
    shell.connect_clipboard();
    #[cfg(feature = "domain-service")]
    shell.connect_service();
    let aliases = alias::load_aliases();

    println!();
    println!("  {} Smart Shell v0.2.0", "●".blue().bold());
    println!("  {} • {}", "SpaceOS".dimmed(), "smart-lib-ng".dimmed());
    println!("  Type 'help' for commands, 'exit' to leave");
    println!("  Tab completion wired, arrow keys for history");
    println!();

    if crate::color::is_tty() {
        run_repl(&mut shell, &aliases);
    } else {
        run_script(&mut shell, &aliases);
    }
}

#[cfg(feature = "domain-network")]
fn run_repl(shell: &mut Shell, aliases: &std::collections::HashMap<String, String>) {
    let helper = ShellHelper {
        completer: Completer::new(),
    };
    let mut rl = rustyline::Editor::<ShellHelper, rustyline::history::DefaultHistory>::new()
        .expect("Failed to create readline editor");
    rl.set_helper(Some(helper));

    let history_path = dirs::home_dir()
        .map(|h| h.join(".config/smart/history"))
        .unwrap_or_else(|| std::path::PathBuf::from(".smart_history"));
    let _ = rl.load_history(&history_path);

    loop {
        let prompt = format!("{} ", "smart>".cyan().bold());
        match rl.readline(&prompt) {
            Ok(input) => {
                let input = input.trim().to_string();
                if input.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(&input);
                eval_line(shell, &alias::process_command_input(&input, aliases));
            }
            Err(rustyline::error::ReadlineError::Interrupted) => println!("^C"),
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("Goodbye!");
                break;
            }
            Err(err) => {
                eprintln!("  Error: {err:?}");
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);
}

#[cfg(not(feature = "domain-network"))]
fn run_repl(shell: &mut Shell, aliases: &std::collections::HashMap<String, String>) {
    // No engine domains compiled: the rustyline editor still runs (the shell
    // language needs no engine), and every domain answers honestly.
    let mut rl = rustyline::DefaultEditor::new().expect("Failed to create readline editor");
    loop {
        let prompt = format!("{} ", "smart>".cyan().bold());
        match rl.readline(&prompt) {
            Ok(input) => {
                let input = input.trim().to_string();
                if input.is_empty() {
                    continue;
                }
                eval_line(shell, &alias::process_command_input(&input, aliases));
            }
            Err(_) => break,
        }
    }
}

/// The scripted mode: stdin is not a terminal, so the same evaluator runs
/// line by line (a block header accumulates its body until a blank line).
fn run_script(shell: &mut Shell, aliases: &std::collections::HashMap<String, String>) {
    use std::io::BufRead;
    // stdin is the PROGRAM here, so a confirmation never reads it: a
    // destructive verb refuses instead of consuming (or blocking on) the very
    // lines this loop is reading.
    shell.set_script_mode(true);
    let stdin = std::io::stdin();
    let mut pending: Option<String> = None;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if let Some(block) = pending.as_mut() {
            if line.trim().is_empty() {
                let block = pending.take().unwrap();
                eval_line(shell, &alias::process_command_input(&block, aliases));
            } else {
                block.push('\n');
                block.push_str(&line);
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("if ") || trimmed.starts_with("for ") || trimmed.starts_with("fn ") {
            pending = Some(trimmed.to_string());
            continue;
        }
        eval_line(shell, &alias::process_command_input(trimmed, aliases));
    }
    if let Some(block) = pending.take() {
        eval_line(shell, &alias::process_command_input(&block, aliases));
    }
}

/// Evaluate one shell-language line under the caller's output shape and gate.
/// Raw prints the value's pipe-ready PLAIN TEXT form (`value::to_raw`) instead
/// of the pretty table — the door the one-shot `-o raw` used to leave shut.
pub fn eval_line(shell: &mut Shell, line: &str) {
    // The REPL's own evaluator keeps the shell's gate: the caller installed it
    // (script mode drives stdin itself, `--force` was passed) and a line
    // evaluation must not quietly drop it.
    let gate = shell.gate;
    eval_line_with(shell, line, crate::OutputFormat::Human, gate);
}

/// Evaluate one line under the caller's output shape. Raw prints the value's
/// pipe-ready PLAIN TEXT form (`value::to_raw`) instead of the pretty table —
/// the door the one-shot `-o raw` used to leave shut. `force` is carried into
/// the shell so a `-c` line can accept a host write explicitly.
pub fn eval_line_with(shell: &mut Shell, line: &str, format: crate::OutputFormat, gate: WriteGate) {
    shell.set_gate(gate);
    let cmd = Shell::parse(line);
    match shell.exec_cmd(&cmd) {
        Ok(value) => {
            if matches!(value, Value::Null) {
                return;
            }
            match format {
                crate::OutputFormat::Raw => print!("{}", crate::value::to_raw(&value)),
                _ => println!("{}", crate::value::render_pretty(&value)),
            }
        }
        Err(e) => {
            if format == crate::OutputFormat::Json {
                println!("{}", crate::error::json_error(&e));
            } else {
                eprintln!("  {} {}", "✗".red(), e);
            }
        }
    }
}
