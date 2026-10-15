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
use crate::{OutputFormat, SmartError, SmartResult};

use crate::domains::network::Backend;
#[cfg(feature = "domain-clipboard")]
use crate::domains::clipboard::Backend as ClipboardBackend;
#[cfg(feature = "domain-service")]
use crate::domains::service::Backend as ServiceBackend;

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
        }
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
                    let backend = smart_lib::platforms::linux::files::LinuxFilesBackend::new();
                    let root = crate::domains::files::selector_root(&backend)?;
                    return Ok(selector::resolve(root, &sel));
                }
                #[cfg(feature = "domain-process")]
                if domain == "process" {
                    let backend = smart_lib::platforms::linux::process::LinuxProcessBackend::new();
                    let root = crate::domains::process::selector_root(&backend)?;
                    return Ok(selector::resolve(root, &sel));
                }
                #[cfg(feature = "domain-settings")]
                if domain == "settings" {
                    let backend = smart_lib::platforms::linux::settings::LinuxSettingsBackend::new();
                    let root = crate::domains::settings::selector_root(&backend)?;
                    return Ok(selector::resolve(root, &sel));
                }
                #[cfg(feature = "domain-logging")]
                if domain == "logging" {
                    let backend = smart_lib::platforms::linux::logging::LinuxLoggingBackend::new();
                    let root = crate::domains::logging::selector_root(&backend)?;
                    return Ok(selector::resolve(root, &sel));
                }
                #[cfg(feature = "domain-clock")]
                if domain == "clock" {
                    let backend = smart_lib::platforms::linux::clock::LinuxClockBackend::new();
                    let root = crate::domains::clock::selector_root(&backend)?;
                    return Ok(selector::resolve(root, &sel));
                }
                #[cfg(feature = "domain-time")]
                if domain == "time" {
                    let backend = smart_lib::platforms::linux::time::LinuxTimeBackend::new();
                    let root = crate::domains::time::selector_root(&backend)?;
                    return Ok(selector::resolve(root, &sel));
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
        let guard = network::WriteGuard { allowed: false };
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
    /// Host-mutating verbs are gated (allowed: false here; the REPL prompts and
    /// one-shot `--force` opens the gate).
    #[cfg(feature = "domain-clipboard")]
    fn exec_clipboard(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::clipboard::{self, ClipboardCmd};

        let mut argv: Vec<String> = vec!["clipboard".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = ClipboardCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let guard = clipboard::WriteGuard { allowed: false };
        if render {
            clipboard::run(&cmd, &OutputFormat::Human, self.clipboard.as_ref(), guard)?;
            Ok(Value::Null)
        } else {
            clipboard::evaluate(&cmd, self.clipboard.as_ref(), guard)
        }
    }

    /// The service domain: parse its command with the same clap grammar the
    /// one-shot binary uses and route it to the supervisor's native read path,
    /// in-process. Host-mutating verbs are gated (allowed: false here; the REPL
    /// prompts and one-shot `--force` opens the gate).
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
        let guard = service::WriteGuard { allowed: false };
        if render {
            service::run(&cmd, &OutputFormat::Human, self.service.as_ref(), guard)?;
            Ok(Value::Null)
        } else {
            service::evaluate(&cmd, self.service.as_ref(), guard)
        }
    }

    /// The files domain: parse with the same clap grammar the one-shot binary
    /// uses and route to the engine, in-process. Host-mutating verbs are gated
    /// (allowed: false here; the REPL prompts and one-shot `--force` opens it).
    #[cfg(feature = "domain-files")]
    fn exec_files(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::files::{self, FilesCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["files".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = FilesCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let backend = smart_lib::platforms::linux::files::LinuxFilesBackend::new();
        let guard = files::WriteGuard { allowed: false };
        if render {
            files::run(&cmd, &OutputFormat::Human, &backend, guard)?;
            Ok(Value::Null)
        } else {
            files::evaluate(&cmd, &backend, guard)
        }
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
        let backend = smart_lib::platforms::linux::process::LinuxProcessBackend::new();
        let guard = process::WriteGuard { allowed: false };
        if render {
            process::run(&cmd, &OutputFormat::Human, &backend, guard)?;
            Ok(Value::Null)
        } else {
            process::evaluate(&cmd, &backend, guard)
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
        let backend = smart_lib::platforms::linux::settings::LinuxSettingsBackend::new();
        let guard = settings::WriteGuard { allowed: false };
        if render {
            settings::run(&cmd, &OutputFormat::Human, &backend, guard)?;
            Ok(Value::Null)
        } else {
            settings::evaluate(&cmd, &backend, guard)
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
        let backend = smart_lib::platforms::linux::logging::LinuxLoggingBackend::new();
        let guard = logging::WriteGuard { allowed: false };
        if render {
            logging::run(&cmd, &OutputFormat::Human, &backend, guard)?;
            Ok(Value::Null)
        } else {
            logging::evaluate(&cmd, &backend, guard)
        }
    }

    /// The clock domain (reads + process-local timerfds; the alarm store's
    /// create/delete are gated behind the write guard in the one-shot path).
    #[cfg(feature = "domain-clock")]
    fn exec_clock(&mut self, args: &str, render: bool) -> SmartResult<Value> {
        use crate::domains::clock::{self, ClockCmd};
        use clap::Parser;
        let mut argv: Vec<String> = vec!["clock".to_string()];
        argv.extend(args.split_whitespace().map(str::to_string));
        let cmd = ClockCmd::try_parse_from(argv)
            .map_err(|e| SmartError::invalid_argument(e.to_string().trim().to_string()))?;
        let backend = smart_lib::platforms::linux::clock::LinuxClockBackend::new();
        let guard = clock::WriteGuard { allowed: false };
        if render {
            clock::run(&cmd, &OutputFormat::Human, &backend, guard)?;
            Ok(Value::Null)
        } else {
            clock::evaluate(&cmd, &backend, guard)
        }
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
        let backend = smart_lib::platforms::linux::time::LinuxTimeBackend::new();
        if render {
            time::run(&cmd, &OutputFormat::Human, &backend)?;
            Ok(Value::Null)
        } else {
            time::evaluate(&cmd, &backend)
        }
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

    // ── help: only what is real ─────────────────────────────────────────────

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
            Some(other) => {
                println!("  {} {} — {}", "◌".dimmed(), other, migrate::pending_reason(other));
            }
            None => {
                println!("  {} Smart Shell v0.2.0 — Commands:", "●".blue().bold());
                println!();
                println!("    {:<24} {}", "network", "Network connectivity (real)");
                #[cfg(feature = "domain-clipboard")]
                println!("    {:<24} {}", "clipboard", "The clipboard (native selections)");
                #[cfg(feature = "domain-service")]
                println!(
                    "    {:<24} {}",
                    "service", "The services the supervisor runs (native supervision read)"
                );
                println!("    {:<24} {}", "migrate", "The domain migration queue");
                println!();
                println!("  {} Special:", "●".blue().bold());
                println!("    @<domain>/<field>     Selector (e.g. @network/interfaces)");
                println!("    <cmd> | where ...     Pipe with filter");
                println!("    <cmd> | sort <f>      Sort by field");
                println!("    <cmd> | take <n>      Take first N items");
                println!("    <cmd> | count         Count items");
                println!("    watch network         Stream the store's real events");
                println!("    apply <file.toml>     Apply a config ([network] section)");
                println!("    export [path]         Export the observed state");
                println!("    migrate               The migration queue as data");
                println!("    help <domain>         Domain-specific help");
                println!();
                println!("  {} Variables:", "●".blue().bold());
                println!("    var = @network/interfaces | count   Assign a piped expression");
                println!("    $var                               Use a variable");
                println!();
                println!("  {} Conditionals:", "●".blue().bold());
                println!("    if @network/networking == false: network status");
                println!("    elif @network/interfaces | count > 2: network interfaces");
                println!("    else: network");
                println!();
                println!("  {} Loops:", "●".blue().bold());
                println!("    for iface in @network/interfaces: network interface $iface");
                println!();
                println!("  {} Functions:", "●".blue().bold());
                println!("    fn name(param): ...    name(args)");
                println!();
                println!(
                    "  {} every other domain is the migration queue — `smart migrate` lists it",
                    "◌".dimmed()
                );
            }
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
    let mut shell = Shell::new(backend);
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

/// Evaluate one shell-language line and render the result: a command's own
/// aesthetic when it rendered itself, the pretty value form otherwise.
pub fn eval_line(shell: &mut Shell, line: &str) {
    let cmd = Shell::parse(line);
    match shell.exec_cmd(&cmd) {
        Ok(value) => {
            if !matches!(value, Value::Null) {
                println!("{}", crate::value::render_pretty(&value));
            }
        }
        Err(e) => eprintln!("  {} {}", "✗".red(), e),
    }
}
