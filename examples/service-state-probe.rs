//! `service-state-probe` — the founder's **privileged exact read** for the
//! service domain's runit cell.
//!
//! ## Why this binary exists (the physical limit, and the answer to it)
//!
//! A runit service's exact state lives in `/run/runit/supervise.<name>/stat` —
//! the supervisor's own words, its own transition clock and its want up/down
//! flags. On this host every `supervise.*` directory is `drwx------ root root`,
//! so uid 1000 gets `EACCES` (`sv status sshd` itself answers "unable to open
//! supervise/ok: access denied"). The **states are not parked behind that
//! wall**: the engine's own runit cell derives up/down, the main pid and the
//! transition clock for every uid from the supervised process truth in `/proc`
//! (the `runsv` processes and their children are readable by all). What only a
//! privileged caller can read is the supervisor's own *exact* words — and this
//! binary is the engine's code path for exactly that, so the founder runs it
//! under `sudo` instead of the library ever hiding behind a permission denial.
//!
//! ## Run it (the founder's command)
//!
//! ```text
//! cargo build --release --example service-state-probe --features domain-service,std
//! sudo ./target/release/examples/service-state-probe --state-root /run/runit
//! sudo ./target/release/examples/service-state-probe --state-root /run/runit sshd
//! ```
//!
//! The binary is a thin CLI over the engine's own `RunitBackend::exact_states`
//! and `render_exact_states`, so the format is test-locked in the library and
//! the tool cannot drift from what the backend reports. Exit codes: `0` every
//! named service read, `1` at least one could not be read (the reason names the
//! exact denied path), `2` no supervisor state directory was reached at all.
//!
//! Reads only, always: this tool never writes a supervise file and never sends
//! a control byte (charter law #8 — the host is read-only).

extern crate std;

use std::process::ExitCode;

use smart_lib::platforms::linux::service::runit::{
    render_exact_states, RunitBackend, DEFAULT_DECLARED_ROOT, DEFAULT_PIDFILE_ROOT,
    DEFAULT_PROC_ROOT, DEFAULT_SCAN_ROOT, DEFAULT_STATE_ROOT,
};

fn main() -> ExitCode {
    let mut state_root = DEFAULT_STATE_ROOT.to_string();
    let mut declared_root = DEFAULT_DECLARED_ROOT.to_string();
    let mut scan_root = DEFAULT_SCAN_ROOT.to_string();
    let mut proc_root = DEFAULT_PROC_ROOT.to_string();
    let mut pidfile_root = DEFAULT_PIDFILE_ROOT.to_string();
    let mut names: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| -> String {
            args.next().unwrap_or_else(|| {
                eprintln!("service-state-probe: {flag} needs a value");
                std::process::exit(2);
            })
        };
        match arg.as_str() {
            "--state-root" => state_root = value("--state-root"),
            "--declared-root" => declared_root = value("--declared-root"),
            "--scan-root" => scan_root = value("--scan-root"),
            "--proc-root" => proc_root = value("--proc-root"),
            "--pidfile-root" => pidfile_root = value("--pidfile-root"),
            "-h" | "--help" => {
                println!(
                    "usage: service-state-probe [--state-root DIR] [--declared-root DIR] \
                     [--scan-root DIR] [--proc-root DIR] [--pidfile-root DIR] [name]\n\
                     The runit supervisor's own exact state read \
                     (/run/runit/supervise.<name>/stat).\n\
                     On this host that read is root-only, so run it under `sudo`:\n  \
                       sudo ./target/release/examples/service-state-probe --state-root /run/runit"
                );
                return ExitCode::SUCCESS;
            }
            other if other.starts_with("--") => {
                eprintln!("service-state-probe: unknown flag {other}");
                return ExitCode::from(2);
            }
            other => names.push(other.to_string()),
        }
    }

    if !std::path::Path::new(&state_root).is_dir() {
        eprintln!(
            "service-state-probe: no supervisor state directory at {state_root} — this is not a \
             runit supervise tree, so there is no exact state to read."
        );
        return ExitCode::from(2);
    }

    let backend = RunitBackend::with_roots(
        std::path::PathBuf::from(scan_root),
        std::path::PathBuf::from(declared_root),
        std::path::PathBuf::from(state_root),
        std::path::PathBuf::from(proc_root),
        std::path::PathBuf::from(pidfile_root),
    );
    // One call per named service (or the whole tree when none is named), so
    // the report's rows stay in the caller's order.
    let mut rows = Vec::new();
    if names.is_empty() {
        rows.extend(backend.exact_states(None));
    } else {
        for name in &names {
            rows.extend(backend.exact_states(Some(name)));
        }
    }
    if rows.is_empty() {
        eprintln!("service-state-probe: none of the named services is declared or linked here");
        return ExitCode::from(2);
    }
    print!("{}", render_exact_states(&rows));
    let unreadable = rows.iter().any(|r| r.stat.is_none());
    if unreadable {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
