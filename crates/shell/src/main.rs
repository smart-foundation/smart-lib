//! The `smart` binary's front-end: the clap grammar (`smart [OPTIONS]
//! [COMMAND]`), with bare `smart` opening the REPL and every domain living in
//! ONE grammar — the domain's full name, no abbreviations.

use clap::Parser;
use std::fmt;

use shell::domains::network::NetworkCmd;
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
    #[arg(long, global = true, help = "Accept host-mutating verbs (network wifi on/off, apply)")]
    pub force: bool,
    /// Run one shell-language line and exit (selectors, pipes, conditionals).
    #[arg(short = 'c', long = "command", value_name = "LINE")]
    pub line: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum DomainCommand {
    /// Network connectivity — the real domain on the new engine.
    Network(NetworkCmd),
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
            DomainCommand::Network(_) => write!(f, "DomainCommand::Network(...)"),
            DomainCommand::Migrate { .. } => write!(f, "DomainCommand::Migrate(...)"),
        }
    }
}

fn main() {
    let cli = Cli::parse();

    // The engine is embedded in-process (the corrected architecture): the
    // shell talks to it through the per-domain backend trait object, exactly
    // as the original shell did.
    #[cfg(feature = "domain-network")]
    let backend = match smart_lib::platforms::linux::network::LinuxNetworkBackend::connect() {
        Ok(b) => Box::new(b) as Box<shell::domains::network::Backend>,
        Err(e) => {
            shell::color::error(&format!(
                "smart: cannot reach the engine's network backend: {e}"
            ));
            eprintln!("(reads only; nothing was changed)");
            std::process::exit(1);
        }
    };

    if let Some(line) = cli.line.as_deref() {
        #[cfg(feature = "domain-network")]
        {
            let mut shell = shell::shell::Shell::new(backend);
            shell::shell::eval_line(&mut shell, line);
            return;
        }
        #[cfg(not(feature = "domain-network"))]
        {
            let _ = line;
            shell::color::error("smart: built without the domain-network feature");
            std::process::exit(1);
        }
    }

    match &cli.command {
        Some(DomainCommand::Network(cmd)) => {
            #[cfg(feature = "domain-network")]
            {
                use shell::domains::network::WriteGuard;
                let guard = WriteGuard { allowed: cli.force };
                match shell::domains::network::run(cmd, &cli.output, backend.as_ref(), guard) {
                    Ok(_) => {}
                    Err(e) => report(e),
                }
            }
            #[cfg(not(feature = "domain-network"))]
            {
                let _ = (cmd, &cli.output);
                shell::color::error(
                    "smart: the network domain is not compiled (build with default features)",
                );
                std::process::exit(1);
            }
        }
        Some(DomainCommand::Migrate { .. }) => {
            print!("{}", migrate::render_queue(&cli.output));
        }
        None => {
            if cli.interactive || cli.command.is_none() {
                #[cfg(feature = "domain-network")]
                run_shell(backend);
                #[cfg(not(feature = "domain-network"))]
                {
                    let _ = &cli;
                    shell::color::error(
                        "smart: the REPL needs the engine's network domain (default features)",
                    );
                    std::process::exit(1);
                }
            }
        }
    }
}

fn report(e: SmartError) -> ! {
    shell::color::error(&format!("smart: {e}"));
    std::process::exit(1);
}
