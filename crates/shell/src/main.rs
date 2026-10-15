//! The `smart` binary's front-end: the clap grammar (`smart [OPTIONS]
//! [COMMAND]`), with bare `smart` opening the REPL and every domain living in
//! ONE grammar — the domain's full name, no abbreviations.

use clap::Parser;
use std::fmt;

#[cfg(feature = "domain-network")]
use shell::domains::network::NetworkCmd;
#[cfg(feature = "domain-clipboard")]
use shell::domains::clipboard::ClipboardCmd;
#[cfg(feature = "domain-service")]
use shell::domains::service::ServiceCmd;
#[cfg(feature = "domain-files")]
use shell::domains::files::FilesCmd;
#[cfg(feature = "domain-process")]
use shell::domains::process::ProcessCmd;
#[cfg(feature = "domain-settings")]
use shell::domains::settings::SettingsCmd;
#[cfg(feature = "domain-logging")]
use shell::domains::logging::LoggingCmd;
#[cfg(feature = "domain-clock")]
use shell::domains::clock::ClockCmd;
#[cfg(feature = "domain-time")]
use shell::domains::time::TimeCmd;
use shell::migrate;
use shell::shell::run_shell_with;
use shell::{OutputFormat, SmartError};

#[derive(Debug, Parser)]
#[command(name = "smart")]
#[command(about = "Smart Protocol shell — one binary hosting every domain", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<DomainCommand>,
    /// The output shape: human (the aesthetic), json (agents/scripts), raw (pipes).
    #[arg(short, long, default_value = "human", global = true)]
    pub output: OutputFormat,
    /// Enter the interactive REPL (the default when no command is given).
    #[arg(short, long, global = true, help = "Enter interactive REPL mode")]
    pub interactive: bool,
    /// Accept the host write a mutating verb performs (the live bring-back
    /// proof is founder-run; charter law #8).
    #[arg(long, global = true, help = "Accept host-mutating verbs (files write/edit/rm, process signal, settings set, network wifi on/off, clipboard set, service start/stop/restart, apply)")]
    pub force: bool,
    /// Print the plan of a host-mutating verb and perform no write.
    #[arg(long, global = true, help = "Show what a mutating verb would do (with the target's type/size/mtime) and perform no write")]
    pub dry_run: bool,
    /// Run one shell-language line and exit (selectors, pipes, conditionals).
    #[arg(short = 'c', long = "command", value_name = "LINE")]
    pub line: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum DomainCommand {
    /// Network connectivity — the real domain on the new engine.
    #[cfg(feature = "domain-network")]
    Network(NetworkCmd),
    /// The system clipboard — the native selection read/write path.
    #[cfg(feature = "domain-clipboard")]
    Clipboard(ClipboardCmd),
    /// The services the host's supervisor runs — observed, not shelled out to.
    #[cfg(feature = "domain-service")]
    Service(ServiceCmd),
    /// The file surface — statx/getdents/inotify/copy_file_range, read natively.
    #[cfg(feature = "domain-files")]
    Files(FilesCmd),
    /// The processes the host runs — /proc/<pid> read natively, measured.
    #[cfg(feature = "domain-process")]
    Process(ProcessCmd),
    /// The host's own settings — the glib schema source and dconf, read live.
    #[cfg(feature = "domain-settings")]
    Settings(SettingsCmd),
    /// The log sinks the host has — svlogd, /var/log, this process's stderr.
    #[cfg(feature = "domain-logging")]
    Logging(LoggingCmd),
    /// The host's clocks and timers — kernel clock_gettime and timerfd.
    #[cfg(feature = "domain-clock")]
    Clock(ClockCmd),
    /// What time it is — the kernel clock, TZif zone, contract calendar math.
    #[cfg(feature = "domain-time")]
    Time(TimeCmd),
    /// The domain migration queue, as data.
    Migrate {
        #[command(subcommand)]
        action: Option<MigrateAction>,
    },
}

#[derive(clap::Subcommand)]
pub enum MigrateAction {
    /// Show the queue (the default).
    Show,
}

impl fmt::Debug for DomainCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "domain-network")]
            DomainCommand::Network(_) => write!(f, "DomainCommand::Network(...)"),
            #[cfg(feature = "domain-clipboard")]
            DomainCommand::Clipboard(_) => write!(f, "DomainCommand::Clipboard(...)"),
            #[cfg(feature = "domain-service")]
            DomainCommand::Service(_) => write!(f, "DomainCommand::Service(...)"),
            #[cfg(feature = "domain-files")]
            DomainCommand::Files(_) => write!(f, "DomainCommand::Files(...)"),
            #[cfg(feature = "domain-process")]
            DomainCommand::Process(_) => write!(f, "DomainCommand::Process(...)"),
            #[cfg(feature = "domain-settings")]
            DomainCommand::Settings(_) => write!(f, "DomainCommand::Settings(...)"),
            #[cfg(feature = "domain-logging")]
            DomainCommand::Logging(_) => write!(f, "DomainCommand::Logging(...)"),
            #[cfg(feature = "domain-clock")]
            DomainCommand::Clock(_) => write!(f, "DomainCommand::Clock(...)"),
            #[cfg(feature = "domain-time")]
            DomainCommand::Time(_) => write!(f, "DomainCommand::Time(...)"),
            DomainCommand::Migrate { .. } => write!(f, "DomainCommand::Migrate(...)"),
        }
    }
}

/// The engine is embedded in-process (the corrected architecture): the shell
/// talks to it through the per-domain backend trait object. The network backend
/// is built only when its feature is on; without it the seam is the unit type,
/// so a clipboard-only build still runs.
#[cfg(feature = "domain-network")]
fn network_backend() -> Box<shell::domains::network::Backend> {
    match smart_lib::platforms::linux::network::LinuxNetworkBackend::connect() {
        Ok(b) => Box::new(b) as Box<shell::domains::network::Backend>,
        Err(e) => {
            shell::color::error(&format!(
                "smart: cannot reach the engine's network backend: {e}"
            ));
            eprintln!("(reads only; nothing was changed)");
            std::process::exit(1);
        }
    }
}
#[cfg(not(feature = "domain-network"))]
fn network_backend() -> Box<shell::domains::network::Backend> {
    Box::new(())
}

fn main() {
    let cli = Cli::parse();
    let backend = network_backend();

    if let Some(line) = cli.line.as_deref() {
        let mut shell = shell::shell::Shell::new(backend);
        // The one-shot line shares the same session surface the REPL uses: the
        // clock-timer ledger is persistent, so a timer armed here leaves an
        // honest ledger row for the next process to name (owned, or gone when
        // this pid is no longer there).
        shell.enable_state_persistence();
        #[cfg(feature = "domain-clipboard")]
        shell.connect_clipboard();
        #[cfg(feature = "domain-service")]
        shell.connect_service();
        shell::shell::eval_line_with(&mut shell, line, cli.output, gate(&cli));
        return;
    }

    match &cli.command {
        #[cfg(feature = "domain-network")]
        Some(DomainCommand::Network(cmd)) => {
            use shell::domains::network::WriteGuard;
            let guard = WriteGuard {
                allowed: accepted(&cli.output, gate(&cli), shell::domains::network::write_intent(cmd), ""),
            };
            match shell::domains::network::run(cmd, &cli.output, backend.as_ref(), guard) {
                Ok(value) => emit(&cli.output, &value),
                Err(e) => report(e, cli.output),
            }
        }
        #[cfg(feature = "domain-clipboard")]
        Some(DomainCommand::Clipboard(cmd)) => {
            use shell::domains::clipboard;
            // Native transport when the host has a seat; the seat-less backend
            // (honest Unavailable-with-reason) otherwise.
            let clipboard_backend = smart_lib::platforms::linux::clipboard::LinuxClipboardBackend::native_connect()
                .unwrap_or_else(|_| {
                    smart_lib::platforms::linux::clipboard::LinuxClipboardBackend::solid()
                });
            let guard = clipboard::WriteGuard {
                allowed: accepted(&cli.output, gate(&cli), clipboard::write_intent(cmd), ""),
            };
            match clipboard::run(cmd, &cli.output, &clipboard_backend, guard) {
                Ok(value) => emit(&cli.output, &value),
                Err(e) => report(e, cli.output),
            }
        }
        #[cfg(feature = "domain-service")]
        Some(DomainCommand::Service(cmd)) => {
            use shell::domains::service;
            // The host's supervisor, measured at construction (runit on this
            // box): every read is its own native path, never a `sv`/`systemctl`
            // subprocess.
            let backend = smart_lib::platforms::linux::service::LinuxServiceBackend::native_connect();
            let guard = service::WriteGuard {
                allowed: accepted(&cli.output, gate(&cli), service::write_intent(cmd), ""),
            };
            match service::run(cmd, &cli.output, &backend, guard) {
                Ok(value) => emit(&cli.output, &value),
                Err(e) => report(e, cli.output),
            }
        }
        #[cfg(feature = "domain-files")]
        Some(DomainCommand::Files(cmd)) => {
            use shell::domains::files;
            let backend = smart_lib::platforms::linux::files::LinuxFilesBackend::new();
            let guard = files::WriteGuard {
                allowed: accepted(&cli.output, gate(&cli), files::write_intent(cmd), &files::write_context(cmd, &backend)),
            };
            // The watch ledger rides with the one-shot command exactly as it
            // rides with a REPL line: a `files watch` records the
            // subscription's fact for as long as the fd is open, and a fresh
            // process names the rows the earlier ones left.
            let mut watches = shell::session::WatchSession::persistent();
            match files::run_session(cmd, &cli.output, &backend, guard, &mut watches) {
                Ok(value) => emit(&cli.output, &value),
                Err(e) => report(e, cli.output),
            }
        }
        #[cfg(feature = "domain-process")]
        Some(DomainCommand::Process(cmd)) => {
            use shell::domains::process;
            let backend = smart_lib::platforms::linux::process::LinuxProcessBackend::new();
            let guard = process::WriteGuard {
                allowed: accepted(&cli.output, gate(&cli), process::write_intent(cmd), ""),
            };
            match process::run(cmd, &cli.output, &backend, guard) {
                Ok(value) => emit(&cli.output, &value),
                Err(e) => report(e, cli.output),
            }
        }
        #[cfg(feature = "domain-settings")]
        Some(DomainCommand::Settings(cmd)) => {
            use shell::domains::settings;
            let backend = smart_lib::platforms::linux::settings::LinuxSettingsBackend::new();
            let guard = settings::WriteGuard {
                allowed: accepted(
                    &cli.output,
                    gate(&cli),
                    settings::write_intent(cmd),
                    &settings::write_context(cmd, &backend),
                ),
            };
            match settings::run(cmd, &cli.output, &backend, guard) {
                Ok(value) => emit(&cli.output, &value),
                Err(e) => report(e, cli.output),
            }
        }
        #[cfg(feature = "domain-logging")]
        Some(DomainCommand::Logging(cmd)) => {
            use shell::domains::logging;
            let backend = smart_lib::platforms::linux::logging::LinuxLoggingBackend::new();
            let guard = logging::WriteGuard {
                allowed: accepted(&cli.output, gate(&cli), logging::write_intent(cmd), ""),
            };
            match logging::run(cmd, &cli.output, &backend, guard) {
                Ok(value) => emit(&cli.output, &value),
                Err(e) => report(e, cli.output),
            }
        }
        #[cfg(feature = "domain-clock")]
        Some(DomainCommand::Clock(cmd)) => {
            use shell::domains::clock;
            let backend = smart_lib::platforms::linux::clock::LinuxClockBackend::new();
            let guard = clock::WriteGuard {
                allowed: accepted(&cli.output, gate(&cli), clock::write_intent(cmd), ""),
            };
            // The clock's timer ledger is a session surface: one-shot creates
            // are recorded beside the session's live timers, and a fresh
            // process reads the ledger the last ones left (never a fabricated
            // firing — the kernel fd died with its owner).
            let mut session = shell::session::ClockSession::persistent();
            match clock::run_session(cmd, &cli.output, &backend, guard, &mut session) {
                Ok(value) => emit(&cli.output, &value),
                Err(e) => report(e, cli.output),
            }
        }
        #[cfg(feature = "domain-time")]
        Some(DomainCommand::Time(cmd)) => {
            use shell::domains::time;
            let backend = smart_lib::platforms::linux::time::LinuxTimeBackend::new();
            match time::run(cmd, &cli.output, &backend) {
                Ok(value) => emit(&cli.output, &value),
                Err(e) => report(e, cli.output),
            }
        }
        Some(DomainCommand::Migrate { .. }) => {
            print!("{}", migrate::render_queue(&cli.output));
        }
        None => {
            if cli.interactive || cli.command.is_none() {
                run_shell_with(backend, gate(&cli));
            }
        }
    }
}

/// The raw door: `-o raw` prints the value's pipe-ready PLAIN TEXT form — a
/// tab-separated table for a list of records, one line per scalar — instead of
/// the 0 bytes it used to print (every domain `run()` skips its own rendering
/// for Raw, and this used to drop the returned value on the floor).
fn emit(format: &shell::OutputFormat, value: &shell::selector::Value) {
    if *format == shell::OutputFormat::Raw {
        print!("{}", shell::value::to_raw(value));
    }
}

/// The founder's confirmation standard, at the one-shot door: a destructive
/// verb asks `delete <path>? (y/N)` with the CONTEXT beside it (the live read's
/// type/size/mtime, or the old→new of a move), an explicit `--force` is the
/// documented acceptance, and a script with no operator and no piped `y` line
/// REFUSES rather than assuming yes. Refusals answer in the caller's format: a
/// JSON error answer under `--json`, the human line otherwise. A read verb
/// carries no intent, so it is never asked anything.
fn accepted(
    format: &shell::OutputFormat,
    gate: shell::shell::WriteGate,
    intent: Option<(String, String)>,
    context: &str,
) -> bool {
    match shell::shell::accept_write(intent, context, gate) {
        Ok(allowed) => allowed,
        Err(e) => report(e, *format),
    }
}

/// The one-shot binary's gate: interactive, so an operator at a terminal is
/// asked and a caller with a piped `y` line answers through stdin. A script
/// with no answer line refuses (a JSON answer under `--json`).
fn gate(cli: &Cli) -> shell::shell::WriteGate {
    shell::shell::WriteGate::interactive(cli.force, cli.dry_run)
}

fn report(e: SmartError, format: shell::OutputFormat) -> ! {
    if format == shell::OutputFormat::Json {
        // Script mode's answer is JSON, with the error's own kind — never a
        // prose line a parser would have to guess at.
        println!("{}", shell::error::json_error(&e));
    } else {
        shell::color::error(&format!("smart: {e}"));
    }
    std::process::exit(1);
}
