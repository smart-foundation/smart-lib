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
use shell::migrate;
use shell::shell::run_shell;
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
    #[arg(long, global = true, help = "Accept host-mutating verbs (network wifi on/off, clipboard set, service start/stop/restart, apply)")]
    pub force: bool,
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
        #[cfg(feature = "domain-clipboard")]
        shell.connect_clipboard();
        #[cfg(feature = "domain-service")]
        shell.connect_service();
        shell::shell::eval_line(&mut shell, line);
        return;
    }

    match &cli.command {
        #[cfg(feature = "domain-network")]
        Some(DomainCommand::Network(cmd)) => {
            use shell::domains::network::WriteGuard;
            let guard = WriteGuard { allowed: cli.force };
            match shell::domains::network::run(cmd, &cli.output, backend.as_ref(), guard) {
                Ok(_) => {}
                Err(e) => report(e),
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
            let guard = clipboard::WriteGuard { allowed: cli.force };
            match clipboard::run(cmd, &cli.output, &clipboard_backend, guard) {
                Ok(_) => {}
                Err(e) => report(e),
            }
        }
        #[cfg(feature = "domain-service")]
        Some(DomainCommand::Service(cmd)) => {
            use shell::domains::service;
            // The host's supervisor, measured at construction (runit on this
            // box): every read is its own native path, never a `sv`/`systemctl`
            // subprocess.
            let backend = smart_lib::platforms::linux::service::LinuxServiceBackend::native_connect();
            let guard = service::WriteGuard { allowed: cli.force };
            match service::run(cmd, &cli.output, &backend, guard) {
                Ok(_) => {}
                Err(e) => report(e),
            }
        }
        Some(DomainCommand::Migrate { .. }) => {
            print!("{}", migrate::render_queue(&cli.output));
        }
        None => {
            if cli.interactive || cli.command.is_none() {
                run_shell(backend);
            }
        }
    }
}

fn report(e: SmartError) -> ! {
    shell::color::error(&format!("smart: {e}"));
    std::process::exit(1);
}
