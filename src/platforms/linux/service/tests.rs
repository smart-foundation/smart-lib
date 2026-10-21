//! The L3 behaviour tests for the Linux service backend — the ones the
//! availability matrix names (`l3_linux_service::…`). Every one of them drives
//! a **scripted supervision tree** (a real directory layout built in a temp
//! dir) or a **scripted bus peer**, so the native read path, the honesty layer,
//! the bounded pulls and the lifecycle mapping are proven without touching the
//! host (charter law #8). The host-touching live probe is `#[ignore]`d and
//! read-only.

extern crate std;

use alloc::ffi::CString;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use std::fs;
use std::path::{Path, PathBuf};

use super::runit::{control_byte, RunitBackend};
use super::NativeSupervision;
use super::{Cell, LinuxServiceBackend};
use crate::core::ids::ServiceId;
use crate::core::ErrorCode;
use crate::domains::service::availability::{claimed_l3_tests, runtime_gate, SERVICE_AVAILABILITY};
use crate::domains::service::backend::{
    LifecycleVerb, PresentedCapability, ServiceBackend, ServiceLifecycle,
};
use crate::domains::service::types::{
    HealthState, Pagination, ServiceEventPayload, ServiceStatus, SubscriptionId,
};

/// A temp path unique to this test run and this test: the tests run in parallel
/// and each builds its own supervision tree, so the path must not be shared.
fn temp_root(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("smart-{tag}-{}-{n}", std::process::id()))
}

/// A scripted runit supervision tree: the layout this host actually has, in a
/// temp directory. Returns the backend and the root (so a test can extend the
/// tree mid-test and clean it up afterwards).
///
/// The tree models the world a **root** caller sees: the supervise `stat`
/// files are readable, so the exact read answers (and there is no `/proc`
/// fixture — the unprivileged path is proven by `state_is_derived_from_proc_truth`).
fn runit_tree() -> (RunitBackend, PathBuf) {
    let root = temp_root("runit");
    let _ = fs::remove_dir_all(&root);
    let declared = root.join("etc/sv");
    let scan = root.join("var/service");
    let state = root.join("run/runit");
    fs::create_dir_all(&declared).unwrap();
    fs::create_dir_all(&scan).unwrap();
    fs::create_dir_all(&state).unwrap();

    // A service the supervisor is running, with a pid and a transition count.
    declare(&declared, "NetworkManager", "");
    supervise(&state, "NetworkManager", "run: /etc/sv/NetworkManager: (pid 4821) 9123s");
    // A service that is linked but down by request.
    declare(&declared, "sshd", "");
    supervise(&state, "sshd", "down: /etc/sv/sshd: 4s, want down");
    // A service the supervisor wants up and does not have (its failed state).
    declare(&declared, "dhcpcd", "");
    supervise(&state, "dhcpcd", "down: /etc/sv/dhcpcd: 2s, normally up, want up");
    // A service that is declared but neither linked nor supervised.
    declare(&declared, "cold", "");
    // A service whose own script names a dependency, and that dependency.
    declare(&declared, "acpid", "#!/bin/sh\nsv check dbus >/dev/null 2>&1 || exit 1\nexec acpid -f\n");
    declare(&declared, "dbus", "");
    supervise(&state, "dbus", "run: /etc/sv/dbus: (pid 812) 9123s");
    // Link the enabled ones into the scan dir, the way the host has it.
    for name in ["NetworkManager", "sshd", "dhcpcd", "acpid", "dbus"] {
        std::os::unix::fs::symlink(declared.join(name), scan.join(name)).unwrap();
    }

    let backend = RunitBackend::with_roots(
        scan,
        declared,
        state,
        root.join("no-proc"),
        root.join("no-pidfiles"),
    );
    (backend, root)
}

/// Declare one service directory under the declared root.
fn declare(declared_root: &Path, name: &str, run_script: &str) {
    let dir = declared_root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("run"),
        if run_script.is_empty() {
            "#!/bin/sh\nexec /bin/true\n".to_string()
        } else {
            format!("{run_script}\n")
        },
    )
    .unwrap();
}

/// One supervise state directory, with the supervisor's own status line.
fn supervise(state_root: &Path, name: &str, stat: &str) {
    let dir = state_root.join(format!("supervise.{name}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("stat"), format!("{stat}\n")).unwrap();
    if let Some(pid) = stat.split("(pid ").nth(1).and_then(|s| s.split(')').next()) {
        fs::write(dir.join("pid"), format!("{pid}\n")).unwrap();
    }
}

/// The whole tree wrapped by the hub backend, so the contract answers are
/// exercised exactly as a caller receives them.
fn hub() -> (LinuxServiceBackend, PathBuf) {
    let (runit, root) = runit_tree();
    (LinuxServiceBackend::with_cell(Cell::Runit(runit)), root)
}

fn resolve_id(backend: &LinuxServiceBackend, name: &str) -> ServiceId {
    let (id, present) = backend.resolve(name).expect("resolve");
    assert!(present, "{name} must resolve on the scripted tree");
    id
}

// ═══════════════════════════════════════════════════════════════════════
// The /proc-derived supervision truth — a fabricated /proc tree the test
// builds itself, so the unprivileged path is proven hermetically (charter
// law #8: no host process is inspected by a test).
// ═══════════════════════════════════════════════════════════════════════

/// One `/proc/<pid>/stat` line, with the fields the supervisor-truth scan
/// reads (state, ppid, starttime) at their kernel field positions.
fn fake_stat(pid: u64, comm: &str, ppid: u64, state: char, starttime_ticks: u64) -> String {
    format!(
        "{pid} ({comm}) {state} {ppid} {pid} {pid} 0 -1 4194560 100 0 0 0 10 5 0 0 20 0 1 0 \
         {starttime_ticks} 100 200 10 0 0 0 0"
    )
}

/// A fixture `/proc` process: its stat line and, when named, its cmdline.
fn fake_process(proc_root: &Path, pid: u64, comm: &str, ppid: u64, state: char, starttime: u64, cmdline: Option<&str>) {
    let dir = proc_root.join(pid.to_string());
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("stat"), format!("{}\n", fake_stat(pid, comm, ppid, state, starttime)))
        .unwrap();
    if let Some(args) = cmdline {
        fs::write(dir.join("cmdline"), args).unwrap();
    }
}

/// The world this host actually presents to **uid 1000**: the supervise state
/// directories exist (they are stat-able), their `stat` files are root-only
/// (mode 000, so the fixture's own owner cannot read them either — the same
/// EACCES the host serves), and the supervised process truth in `/proc` is
/// fully readable. The tree's services are the host's own shape: a running
/// service with a logger, a running one without, a down one with a dead pid
/// file, a multi-process one, a process running outside the supervisor, a
/// zombie child, an empty slot and a cold declaration.
fn root_only_tree() -> (RunitBackend, PathBuf) {
    let root = temp_root("runit-proc");
    let _ = fs::remove_dir_all(&root);
    let declared = root.join("etc/sv");
    let scan = root.join("var/service");
    let state = root.join("run/runit");
    let proc = root.join("proc");
    let pidfiles = root.join("run");
    fs::create_dir_all(&declared).unwrap();
    fs::create_dir_all(&scan).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&proc).unwrap();
    fs::create_dir_all(&pidfiles).unwrap();

    // The kernel's own two clocks: the boot time and the uptime.
    fs::write(proc.join("stat"), "cpu  100 0 100 1000 0 0 0 0 0 0\nbtime 1760000000\n").unwrap();
    fs::write(proc.join("uptime"), "86400.00 172800.00\n").unwrap();

    // runsvdir, the parent of every runsv (the host's pid 677).
    fake_process(&proc, 677, "runsvdir", 1, 'S', 500, Some("runsvdir\0/run/runit/runsvdir/current\0"));

    // sshd: runsv 702, a log child (725) and the service child (726). The
    // service's own pid file names the same pid the process tree does.
    declare(&declared, "sshd", "#!/bin/sh\nexec /usr/bin/sshd -D\n");
    declare_logger(&declared, "sshd", "#!/bin/sh\nexec vlogger -t sshd -p daemon\n");
    supervise_denied(&state, "sshd");
    fake_process(&proc, 702, "runsv", 677, 'S', 900, Some("runsv\0sshd\0"));
    fake_process(&proc, 725, "vlogger", 702, 'S', 901, Some("vlogger\0-t sshd\0-p daemon\0"));
    fake_process(&proc, 726, "sshd", 702, 'S', 901, Some("sshd: /usr/bin/sshd\0-D\0"));
    fs::write(pidfiles.join("sshd.pid"), "726\n").unwrap();

    // elogind: runsv 699 supervises it, only its logger is alive, and its own
    // pid file names a process /proc no longer lists.
    declare(&declared, "elogind", "#!/bin/sh\nexec elogind\n");
    declare_logger(&declared, "elogind", "#!/bin/sh\nexec vlogger -t elogind -p daemon\n");
    supervise_denied(&state, "elogind");
    fake_process(&proc, 699, "runsv", 677, 'S', 905, Some("runsv\0elogind\0"));
    fake_process(&proc, 719, "vlogger", 699, 'S', 906, Some("vlogger\0-t elogind\0-p daemon\0"));
    fs::write(pidfiles.join("elogind.pid"), "1337\n").unwrap();

    // dbus: runsv 696, a log child and the daemon child.
    declare(&declared, "dbus", "#!/bin/sh\nexec dbus-daemon --system --nofork --nopidfile\n");
    declare_logger(&declared, "dbus", "#!/bin/sh\nexec vlogger -t dbus -p daemon\n");
    supervise_denied(&state, "dbus");
    fake_process(&proc, 696, "runsv", 677, 'S', 910, Some("runsv\0dbus\0"));
    fake_process(&proc, 720, "vlogger", 696, 'S', 911, Some("vlogger\0-t dbus\0-p daemon\0"));
    fake_process(&proc, 723, "dbus-daemon", 696, 'S', 911, Some("dbus-daemon\0--system\0--nofork\0--nopidfile\0"));

    // agetty-tty1: one child, no log script at all — the single child is the
    // service process without any script parsing.
    declare(&declared, "agetty-tty1", "#!/bin/sh\nexec chpst -P agetty tty1 38400 linux\n");
    supervise_denied(&state, "agetty-tty1");
    fake_process(&proc, 693, "runsv", 677, 'S', 915, Some("runsv\0agetty-tty1\0"));
    fake_process(&proc, 710, "agetty", 693, 'S', 916, Some("agetty\0--noclear\0tty1\038400\0linux\0"));

    // alsa: the run script execs through chpst (`chpst -b alsa pause`), so the
    // service process's argv[0] is `pause` and the logger is the vlogger.
    declare(&declared, "alsa", "#!/bin/sh\nset -e\nalsactl restore\nexec chpst -b alsa pause\n");
    declare_logger(&declared, "alsa", "#!/bin/sh\nexec vlogger -t alsa -p daemon\n");
    supervise_denied(&state, "alsa");
    fake_process(&proc, 685, "runsv", 677, 'S', 920, Some("runsv\0alsa\0"));
    fake_process(&proc, 704, "vlogger", 685, 'S', 921, Some("vlogger\0-t alsa\0-p daemon\0"));
    fake_process(&proc, 708, "pause", 685, 'S', 921, Some("alsa\0"));

    // forky: two service children and no log script — a wrapper run script,
    // so /proc cannot say which child is the main one.
    declare(&declared, "forky", "#!/bin/sh\nmy-app --worker &\nmy-app --server &\n");
    supervise_denied(&state, "forky");
    fake_process(&proc, 800, "runsv", 677, 'S', 925, Some("runsv\0forky\0"));
    fake_process(&proc, 801, "my-app", 800, 'S', 926, Some("my-app\0--worker\0"));
    fake_process(&proc, 802, "my-app", 800, 'S', 926, Some("my-app\0--server\0"));

    // zombied: the supervisor's child exists but is a zombie — a process that
    // has exited, which is NOT a running service.
    declare(&declared, "zombied", "#!/bin/sh\nexec zombied -D\n");
    supervise_denied(&state, "zombied");
    fake_process(&proc, 810, "runsv", 677, 'S', 930, Some("runsv\0zombied\0"));
    fake_process(&proc, 811, "zombied", 810, 'Z', 931, Some("zombied\0-D\0"));

    // strayd: no runsv supervises it and the supervisor has no state dir for
    // it, but its own pid file names a live process — it runs outside the
    // supervisor, and the answer says exactly that.
    declare(&declared, "strayd", "#!/bin/sh\nexec strayd\n");
    fs::write(pidfiles.join("strayd.pid"), "850\n").unwrap();
    fake_process(&proc, 850, "strayd", 1, 'S', 935, Some("strayd\0"));

    // gone: the supervisor's state directory exists but no runsv process for
    // it is in /proc, and its pid file names a dead process.
    declare(&declared, "gone", "#!/bin/sh\nexec gone -D\n");
    supervise_denied(&state, "gone");
    fs::write(pidfiles.join("gone.pid"), "4242\n").unwrap();

    // netdep: its own script declares the dependency, and the dependency is
    // running on this tree — so the dependency edge measures SATISFIED from
    // the /proc truth, with no stat file readable anywhere.
    declare(&declared, "netdep", "#!/bin/sh\nsv check dbus >/dev/null || exit 1\nexec netdep -D\n");
    supervise_denied(&state, "netdep");
    fake_process(&proc, 820, "runsv", 677, 'S', 945, Some("runsv\0netdep\0"));
    fake_process(&proc, 821, "netdep", 820, 'S', 946, Some("netdep\0-D\0"));

    // cold: declared, not linked, no state dir, no runsv, no pid file.
    declare(&declared, "cold", "#!/bin/sh\nexec /bin/true\n");

    // A runsv whose service the tree does not declare — a phantom supervisor
    // that must not become a service.
    fake_process(&proc, 900, "runsv", 677, 'S', 940, Some("runsv\0phantom\0"));

    // Link the enabled ones, the way the host's scan dir does.
    for name in [
        "sshd", "elogind", "dbus", "agetty-tty1", "alsa", "forky", "zombied", "gone", "netdep",
    ] {
        std::os::unix::fs::symlink(declared.join(name), scan.join(name)).unwrap();
    }

    let backend = RunitBackend::with_roots(scan, declared, state, proc, pidfiles);
    (backend, root)
}

/// Declare one service's log script (the program whose argv[0] is how the log
/// child is told from the service child in /proc).
fn declare_logger(declared_root: &Path, name: &str, log_script: &str) {
    let dir = declared_root.join(name).join("log");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("run"), log_script).unwrap();
}

/// One supervise state directory whose `stat` file is root-only — the host's
/// own permission layout, reproduced for the fixture's unprivileged reader by
/// a mode the fixture's owner cannot read either (uid 1000 has no
/// CAP_DAC_OVERRIDE, so a 0600 file owned by another user is the same EACCES
/// a root-owned `drwx------ supervise.<name>` serves; mode 000 reproduces the
/// denial exactly).
fn supervise_denied(state_root: &Path, name: &str) {
    let dir = state_root.join(format!("supervise.{name}"));
    fs::create_dir_all(&dir).unwrap();
    let stat = dir.join("stat");
    fs::write(&stat, "run: /etc/sv/nobody: (pid 1) 1s\n").unwrap();
    std::fs::set_permissions(&stat, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
}

/// The unprivileged runit state read, proven over a fabricated `/proc`: every
/// supervised service answers a REAL state for a caller who cannot read a
/// single `supervise.stat` file — up, down, and why.
#[test]
fn state_is_derived_from_proc_truth_for_every_uid() {
    let (runit, root) = root_only_tree();
    let backend = LinuxServiceBackend::with_cell(Cell::Runit(runit));
    let page = backend
        .list_services(Pagination { offset: 0, limit: u32::MAX })
        .expect("listServices");
    let by_name = |name: &str| {
        page.items
            .iter()
            .find(|i| i.name == name)
            .unwrap_or_else(|| panic!("{name} must be enumerated"))
            .clone()
    };
    let state = |name: &str| by_name(name).state;

    // A supervised service with a logger: the service's own child is the main
    // process, the logger is not mistaken for it, and the supervisor's own pid
    // and child count travel with the state.
    let sshd = state("sshd");
    assert_eq!(sshd.status, ServiceStatus::Active);
    assert_eq!(sshd.main_pid, 726);
    assert!(sshd.since_valid, "the process start time is a real observation");
    assert!(sshd.since > 0 && sshd.since_us > 0, "the boot clock is measured, not invented");
    let sshd_meta = &by_name("sshd").state.metadata;
    assert!(sshd_meta.iter().any(|(k, v)| k == "state_source" && v == "proc"));
    assert!(sshd_meta.iter().any(|(k, v)| k == "runsv" && v == "702"));
    assert!(sshd_meta.iter().any(|(k, v)| k == "live_children" && v == "2"));

    // dbus: same shape, the daemon child is the main process.
    assert_eq!(state("dbus").main_pid, 723);

    // agetty-tty1: one child and no log script — no parsing needed.
    assert_eq!(state("agetty-tty1").main_pid, 710);

    // alsa: `exec chpst -b alsa pause` means the service process is `pause`,
    // and the vlogger child is the logger — not the service.
    assert_eq!(state("alsa").main_pid, 708);

    // elogind: supervised, only the logger alive, and its own pid file names
    // a process /proc no longer lists — the down state with the measured fact.
    let elogind = state("elogind");
    assert_eq!(elogind.status, ServiceStatus::Inactive);
    assert_eq!(elogind.main_pid, 0);
    assert!(elogind.reason.contains("only its log process is running"), "{}", elogind.reason);
    assert!(elogind.reason.contains("elogind.pid"), "{}", elogind.reason);
    assert!(elogind.reason.contains("no longer lists"), "{}", elogind.reason);

    // zombied: a child that has exited is not a running service.
    let zombied = state("zombied");
    assert_eq!(zombied.status, ServiceStatus::Inactive);
    assert!(zombied.reason.contains("no supervised process is running"), "{}", zombied.reason);

    // forky: processes ARE running, and the answer says so without pretending
    // to know which one is the main process.
    let forky = state("forky");
    assert_eq!(forky.status, ServiceStatus::Active);
    assert_eq!(forky.main_pid, 0, "no main pid is invented for a wrapper script");
    assert!(forky.reason.contains("2 supervised processes"), "{}", forky.reason);

    // strayd: running outside the supervisor — active, with the reason naming
    // that no supervisor watches it.
    let strayd = state("strayd");
    assert_eq!(strayd.status, ServiceStatus::Active);
    assert_eq!(strayd.main_pid, 850);
    assert!(strayd.reason.contains("no runsv process supervises"), "{}", strayd.reason);

    // gone: the supervisor's slot exists, its process does not.
    let gone = state("gone");
    assert_eq!(gone.status, ServiceStatus::Inactive);
    assert!(gone.reason.contains("no runsv process for gone is in /proc"), "{}", gone.reason);

    // cold: no supervision and no process — the honest unknown naming the
    // denied stat path AND the declaration truth, never a fabricated state.
    let cold = state("cold");
    assert_eq!(cold.status, ServiceStatus::Unknown);
    assert!(cold.reason.contains("supervise.cold/stat"), "{}", cold.reason);
    assert!(cold.reason.contains("not linked into the supervisor's scan dir"), "{}", cold.reason);

    // The phantom runsv is not a service.
    assert!(!page.items.iter().any(|i| i.name == "phantom"));

    // A dependency the service's own script declares is measured satisfied
    // from the same /proc truth (dbus runs on this tree), with no readable
    // stat file anywhere — an unprivileged caller still measures the edge.
    let netdep = backend
        .get_dependencies(resolve_id(&backend, "netdep"))
        .expect("getDependencies");
    assert_eq!(netdep.len(), 1);
    assert_eq!(netdep[0].service_id, resolve_id(&backend, "dbus"));
    assert!(netdep[0].satisfied, "dbus is running on this tree, and /proc says so");

    // A dependency that is not running measures unsatisfied — never assumed.
    declare(&root.join("etc/sv"), "needsgone", "#!/bin/sh\nsv check gone >/dev/null || exit 1\nexec needsgone\n");
    let needsgone = backend
        .get_dependencies(resolve_id(&backend, "needsgone"))
        .expect("getDependencies");
    assert_eq!(needsgone.len(), 1);
    assert!(!needsgone[0].satisfied, "gone is not running, so its edge is not satisfied");

    // The published caps are the measurements of this world: real states for
    // every uid, the exact (privileged) half measured false, control false.
    let caps = backend.supervision_caps();
    assert!(caps.can_read_state, "the /proc truth answers for this unprivileged caller");
    assert!(!caps.can_read_exact_state, "every stat file was denied on this tree");
    assert!(!caps.can_control, "no control fifo exists on this tree");

    // So the state reads are ungated at runtime: nothing is parked unknown.
    for method in ["listServices", "getService", "getState", "getHealth", "listHealth"] {
        assert_eq!(runtime_gate(&caps, method), None, "{method} answers for this uid");
    }
    let _ = fs::remove_dir_all(&root);
}

/// The privileged exact read (`supervise.stat`) is code-complete and proven:
/// the founder's helper prints exactly this over the engine's own path.
#[test]
fn the_exact_supervise_read_is_code_complete() {
    let root = temp_root("runit-exact");
    let _ = fs::remove_dir_all(&root);
    let declared = root.join("etc/sv");
    let scan = root.join("var/service");
    let state = root.join("run/runit");
    fs::create_dir_all(&declared).unwrap();
    fs::create_dir_all(&scan).unwrap();
    fs::create_dir_all(&state).unwrap();
    // readable: `runsv` is up with the supervisor's own pid and clock.
    declare(&declared, "up", "");
    supervise(&state, "up", "run: /etc/sv/up: (pid 4242) 5678s, normally up");
    // denied: the host's own root-only layout, reproduced for an unprivileged
    // reader.
    declare(&declared, "denied", "");
    supervise_denied(&state, "denied");
    // absent: the supervisor has no state file for it.
    declare(&declared, "absent", "");
    for name in ["up", "denied", "absent"] {
        std::os::unix::fs::symlink(declared.join(name), scan.join(name)).unwrap();
    }
    let backend =
        RunitBackend::with_roots(scan, declared, state, root.join("no-proc"), root.join("no-pidfiles"));

    let rows = backend.exact_states(None);
    assert_eq!(rows.len(), 3, "exactly the tree's services");
    let row = |name: &str| rows.iter().find(|r| r.name == name).unwrap().clone();

    let up = row("up");
    assert_eq!(up.status, Some(ServiceStatus::Active));
    assert_eq!(up.main_pid, 4242);
    assert_eq!(up.since_us, 5_678_000_000);
    assert!(up.stat.as_deref().is_some_and(|s| s.contains("normally up")));
    assert!(up.reason.is_none());

    let denied = row("denied");
    assert!(denied.stat.is_none());
    let reason = denied.reason.as_deref().unwrap_or_default();
    assert!(reason.starts_with("no-privilege:"), "{reason}");
    assert!(reason.contains("supervise.denied/stat"), "{reason}");

    let absent = row("absent");
    assert!(absent.reason.as_deref().is_some_and(|r| r.starts_with("not-found:")), "{:?}", absent.reason);

    // A name filter narrows the probe (the founder's per-service check).
    assert_eq!(backend.exact_states(Some("up")).len(), 1);

    // The render is the engine's own, so the helper binary is pure arg parsing.
    let report = super::runit::render_exact_states(&rows);
    assert!(report.starts_with("runit exact state read: 3 service(s)"), "{report}");
    assert!(report.contains("up"), "{report}");
    assert!(report.contains("(pid 4242) 5678s"), "{report}");
    assert!(report.contains("unreadable"), "{report}");
    assert!(report.contains("supervise.denied/stat"), "{report}");
    let _ = fs::remove_dir_all(&root);
}

/// `listServices` is served natively: the scripted tree's services, their
/// measured enablement and their measured state.
#[test]
fn list_services_enumerates_the_supervision_tree_natively() {
    let (backend, root) = hub();
    let page = backend
        .list_services(Pagination { offset: 0, limit: u32::MAX })
        .expect("listServices");
    assert_eq!(page.total, 6, "the scripted tree declares six services");
    let names: Vec<&str> = page.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["NetworkManager", "acpid", "cold", "dbus", "dhcpcd", "sshd"]
    );

    // The measured columns: linked, declared, supervised.
    let nm = page.items.iter().find(|i| i.name == "NetworkManager").unwrap();
    assert!(nm.enabled, "linked into the scan dir");
    assert_eq!(nm.state.status, ServiceStatus::Active);
    let cold = page.items.iter().find(|i| i.name == "cold").unwrap();
    assert!(!cold.enabled, "declared but never linked");
    assert_eq!(cold.state.status, ServiceStatus::Unknown);
    assert!(
        cold.state.reason.contains("supervise.cold/stat"),
        "the honest reason names the path that was not there: {}",
        cold.state.reason
    );

    // Pagination is the contract's: the caller's bound honoured, the total
    // stable, and a limit of 0 means "one".
    let first = backend
        .list_services(Pagination { offset: 0, limit: 2 })
        .expect("listServices");
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.total, 6, "the total is the full enumeration, not the page");
    let second = backend
        .list_services(Pagination { offset: 2, limit: 2 })
        .expect("listServices");
    assert_eq!(second.items.len(), 2);
    assert_eq!(second.items[0].name, "cold");
    let one = backend
        .list_services(Pagination { offset: 0, limit: 0 })
        .expect("listServices");
    assert_eq!(one.items.len(), 1, "a limit of 0 means one, never all");

    assert!(SERVICE_AVAILABILITY.iter().any(|r| r.l3_test == Some(
        "l3_linux_service::list_services_enumerates_the_supervision_tree_natively"
    )));
    let _ = fs::remove_dir_all(&root);
}

/// `getService` reads one entry natively.
#[test]
fn get_service_reads_one_entry_natively() {
    let (backend, root) = hub();
    let nm = resolve_id(&backend, "NetworkManager");
    let info = backend.get_service(nm).expect("getService");
    assert_eq!(info.name, "NetworkManager");
    assert!(info.enabled);
    assert_eq!(info.state.status, ServiceStatus::Active);
    assert_eq!(info.state.main_pid, 4821, "the supervisor's own pid line");
    assert_eq!(
        info.state.since_us, 9_123_000_000,
        "the supervisor's own seconds count, in the contract's unit"
    );
    assert!(info.state.since_valid);

    // An id the supervisor does not serve is a not-found that names the
    // supervisor, never a fabricated service.
    let err = backend.get_service(ServiceId::new(424_242)).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.message.contains("runit"), "{err}");
    let _ = fs::remove_dir_all(&root);
}

/// `getState` parses the supervisor's own status grammar, including the failed
/// and the inactive readings, and keeps the supervisor's words in the reason.
#[test]
fn get_state_parses_the_supervise_status_natively() {
    let (backend, root) = hub();

    let nm = resolve_id(&backend, "NetworkManager");
    let state = backend.get_state(nm).expect("getState");
    assert_eq!(state.status, ServiceStatus::Active);
    assert_eq!(state.main_pid, 4821);
    assert!(state.reason.is_empty(), "an active service needs no reason");

    let sshd = resolve_id(&backend, "sshd");
    let state = backend.get_state(sshd).expect("getState");
    assert_eq!(state.status, ServiceStatus::Inactive);
    let reason = state.reason.clone();
    assert!(state.reason.contains("want down"), "{reason}");

    let dhcpcd = resolve_id(&backend, "dhcpcd");
    let state = backend.get_state(dhcpcd).expect("getState");
    assert_eq!(
        state.status,
        ServiceStatus::Failed,
        "a service the supervisor wants up and does not have is the failed reading"
    );
    let reason = state.reason.clone();
    assert!(state.reason.contains("wants the service up"), "{reason}");

    // The metadata carries the measured supervision facts, so a caller can see
    // what the backend observed beside the status.
    assert_eq!(state.metadata[0].0, "supervisor");
    assert_eq!(state.metadata[0].1, "runit");
    assert_eq!(state.metadata[1].0, "declared");
    let _ = fs::remove_dir_all(&root);
}

/// The status grammar parses the supervisor's own words for every state it
/// writes, and an unknown word stays unknown instead of being guessed.
#[test]
fn the_supervise_status_grammar_parses_the_supervisors_own_words() {
    use super::runit::StatLine;
    let line = StatLine::parse("run: /etc/sv/sshd: (pid 1234) 5678s, normally down");
    assert_eq!(line.word, "run");
    assert_eq!(line.main_pid, 1234);
    assert_eq!(line.since_us, 5_678_000_000);
    assert_eq!(line.flags, "normally down");
    assert_eq!(line.status().0, ServiceStatus::Active);

    let line = StatLine::parse("down: /etc/sv/sshd: 4s, want down");
    assert_eq!(line.main_pid, 0);
    assert_eq!(line.status().0, ServiceStatus::Inactive);

    let line = StatLine::parse("finish: /etc/sv/sshd: 1s");
    assert_eq!(line.status().0, ServiceStatus::Deactivating);

    let line = StatLine::parse("exit: /etc/sv/sshd: 1s");
    assert_eq!(line.status().0, ServiceStatus::Failed);

    // An unrecognised word keeps the supervisor's text in the reason.
    let line = StatLine::parse("wibble: something the backend does not map");
    let (status, reason) = line.status();
    assert_eq!(status, ServiceStatus::Unknown);
    assert!(reason.contains("wibble"), "{reason}");
}

/// `resolve` maps a name to an identity, and an unknown name is the answer
/// rather than a fault.
#[test]
fn resolve_maps_a_name_to_an_identity() {
    let (backend, root) = hub();
    let (id, present) = backend.resolve("NetworkManager").expect("resolve");
    assert!(present);
    // The same name resolves to the same id twice (the stable-total promise).
    let (again, _) = backend.resolve("NetworkManager").expect("resolve");
    assert_eq!(id, again);

    let (absent, present) = backend.resolve("telepathy").expect("resolve");
    assert!(!present, "an unknown name is the answer, not an error");
    assert_eq!(absent.get(), 0);
    let _ = fs::remove_dir_all(&root);
}

/// `getHealth` reports the measured lifecycle and declares the probe unrun — a
/// read-only backend never executes a service's probe script.
#[test]
fn health_reports_the_lifecycle_and_declares_the_probe_unrun() {
    let (backend, root) = hub();
    let nm = resolve_id(&backend, "NetworkManager");
    let health = backend.get_health(nm).expect("getHealth");
    assert_eq!(health.state, HealthState::Online);
    assert!(!health.last_check_passed, "the probe is honestly declared unrun");
    assert_eq!(health.last_check_at, 0, "no check was taken");
    assert!(!health.quarantined);
    assert_eq!(health.restarts, 0, "a supervisor observes no restart counter");

    let dhcpcd = resolve_id(&backend, "dhcpcd");
    let health = backend.get_health(dhcpcd).expect("getHealth");
    assert_eq!(health.state, HealthState::Offline);
    let reason = health.reason.clone();
    assert!(!health.reason.is_empty(), "{reason}");
    let _ = fs::remove_dir_all(&root);
}

/// `listHealth` is the same measurement over the listing: every service the
/// supervisor reports carries a state and an honest reason.
#[test]
fn list_health_is_the_same_measurement_over_the_listing() {
    let (backend, root) = hub();
    let page = backend
        .list_health(Pagination { offset: 0, limit: u32::MAX })
        .expect("listHealth");
    assert_eq!(page.total, 6);
    for health in &page.items {
        // A service that is not online names WHY it is not; an online one
        // needs no excuse, and the reason stays empty rather than invented.
        if health.state != HealthState::Online {
            assert!(
                !health.reason.is_empty(),
                "a health that says nothing names no reason: {health:?}"
            );
        }
    }
    let _ = fs::remove_dir_all(&root);
}

/// `listQuarantined` is the honest empty on a host supervisor: it has no
/// quarantine concept, so the list is empty with a zero total — never a
/// fabricated quarantined entry.
#[test]
fn quarantine_is_an_honest_empty_on_a_host_supervisor() {
    let (backend, root) = hub();
    let page = backend
        .list_quarantined(Pagination { offset: 0, limit: u32::MAX })
        .expect("listQuarantined");
    assert!(page.items.is_empty());
    assert_eq!(page.total, 0);
    let _ = fs::remove_dir_all(&root);
}

/// The concepts a host supervisor does not own answer `Unavailable` with the
/// exact reason class the matrix promises.
#[test]
fn the_measurement_and_registry_surfaces_answer_unavailable_with_a_class() {
    let (backend, root) = hub();
    let nm = resolve_id(&backend, "NetworkManager");
    let err = backend.get_resource_allocation(nm).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(err.message.starts_with("not-applicable:"), "{err}");
    assert!(err.message.contains("quota"), "{err}");

    let err = backend.get_health_check(nm).unwrap_err();
    assert!(err.message.starts_with("not-applicable:"), "{err}");

    let err = backend.get_metrics(nm, 1_000_000_000).unwrap_err();
    assert!(err.message.starts_with("not-applicable:"), "{err}");
    assert!(err.message.contains("request counts"), "{err}");
    let _ = fs::remove_dir_all(&root);
}

/// `getDependencies` reads the supervisor's own declaration: runit's dependency
/// lives in the service's own scripts.
#[test]
fn dependencies_are_read_from_the_service_dirs_own_scripts() {
    let (backend, root) = hub();
    let acpid = resolve_id(&backend, "acpid");
    let deps = backend.get_dependencies(acpid).expect("getDependencies");
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].service_id, resolve_id(&backend, "dbus"));
    assert!(
        deps[0].satisfied,
        "dbus is running on the scripted tree, and the supervisor says so"
    );

    // A service that declares none answers empty — the supervisor's own facts,
    // not a fabricated edge.
    let nm = resolve_id(&backend, "NetworkManager");
    assert!(backend.get_dependencies(nm).expect("getDependencies").is_empty());
    let _ = fs::remove_dir_all(&root);
}

/// A state subscription observes the supervisor: the first pull is what exists,
/// a later pull reports only what changed, and each pull is bounded by the
/// caller.

/// `subscribeState` mints the engine's own subscription token, and the state
/// pull accepts it.
#[test]
fn subscribe_state_mints_a_subscription() {
    let (backend, root) = hub();
    let sub = backend.subscribe_state(ServiceId::new(0)).expect("subscribeState");
    assert!(backend.pull_state(sub, 1).is_ok());
    let _ = fs::remove_dir_all(&root);
}

/// The state pull reports what changed since the last pull, each pull is
/// bounded by the caller, and an unknown subscription id is a not-found.
#[test]
fn a_state_pull_reports_only_what_changed() {
    let (backend, root) = hub();
    let sub = backend.subscribe_state(ServiceId::new(0)).expect("subscribeState");

    let first = backend.pull_state(sub, u32::MAX).expect("pullState");
    assert_eq!(first.len(), 6, "the first pull reports what exists");
    // A state that carries a timestamp carries a MEASURED one: the exact read
    // uses the supervisor's own clock, and the /proc derivation uses the
    // process start time. A state with no observed transition is the honest
    // `since_valid == false` paired with a reason — never a fabricated clock.
    for state in first.iter() {
        if state.since_valid {
            assert!(state.since > 0, "{}: a valid since is measured", state.reason);
        } else {
            assert!(!state.reason.is_empty(), "a state without a clock names why");
        }
    }

    // With nothing changed, the next pull is an empty batch — not an invented
    // event.
    let second = backend.pull_state(sub, u32::MAX).expect("pullState");
    assert!(second.is_empty(), "no transition, no event");

    // A bound the caller states is honoured.
    let bounded = backend.pull_state(sub, 2).expect("pullState");
    assert!(bounded.len() <= 2);

    // And a change the supervisor made between pulls is reported by the next
    // pull, with the supervisor's own words in the reason.
    // The supervisor brings sshd up between the two pulls: the same stat file
    // the tree was built with, rewritten in the supervisor's own grammar.
    let state_root = root.join("run/runit");
    let stat = state_root.join("supervise.sshd/stat");
    fs::write(stat, "run: /etc/sv/sshd: (pid 900) 1s\n").unwrap();
    let after = backend.pull_state(sub, u32::MAX).expect("pullState");
    assert!(
        after.iter().any(|s| s.status == ServiceStatus::Active && s.main_pid == 900),
        "the supervisor's new transition is reported"
    );

    // An unknown subscription id is a not-found, never a panic.
    let err = backend.pull_state(SubscriptionId(9_999), 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let _ = fs::remove_dir_all(&root);
}

/// `cancel` retires the token: a retired subscription is inert and cancelling
/// it again is a no-op rather than an error.

/// `cancelState` retires the state subscription: a retired token is inert,
/// and cancelling it again is a no-op rather than an error.
#[test]
fn cancel_state_retires_the_state_token() {
    let (backend, root) = hub();
    let sub = backend.subscribe_state(ServiceId::new(0)).expect("subscribeState");
    backend.cancel_state(sub).expect("cancelState");
    assert!(backend.cancel_state(sub).is_ok(), "cancelling a retired token is inert");
    let _ = fs::remove_dir_all(&root);
}

/// `cancelServiceEvents` retires the events subscription: a retired token is
/// inert, and cancelling it again is a no-op rather than an error.
#[test]
fn cancel_service_events_retires_the_events_token() {
    let (backend, root) = hub();
    let sub = backend
        .subscribe_service_events(ServiceId::new(0))
        .expect("subscribeServiceEvents");
    backend.cancel_service_events(sub).expect("cancelServiceEvents");
    assert!(
        backend.cancel_service_events(sub).is_ok(),
        "cancelling a retired token is inert"
    );
    let _ = fs::remove_dir_all(&root);
}

/// `subscribeServiceEvents` observes the contract's own event vocabulary from
/// the supervisor's measured transitions.

/// `subscribeServiceEvents` mints the engine's own subscription token, and
/// the events pull accepts it.
#[test]
fn subscribe_service_events_mints_a_subscription() {
    let (backend, root) = hub();
    let sub = backend
        .subscribe_service_events(ServiceId::new(0))
        .expect("subscribeServiceEvents");
    assert!(backend.pull_service_events(sub, 1).is_ok());
    let _ = fs::remove_dir_all(&root);
}

/// The events pull reports the contract's own event vocabulary from the
/// supervisor's measured transitions, and it is empty until something
/// changes.
#[test]
fn a_service_events_pull_reports_the_contract_vocabulary() {
    let (backend, root) = hub();
    let sub = backend
        .subscribe_service_events(ServiceId::new(0))
        .expect("subscribeServiceEvents");
    let events = backend.pull_service_events(sub, u32::MAX).expect("pull");
    assert_eq!(events.len(), 6, "the first pull reports what exists");

    let running = events
        .iter()
        .find(|e| {
            matches!(&e.payload, ServiceEventPayload::StateChanged(s) if s.status == ServiceStatus::Active)
        })
        .expect("an active service yields a started event");
    assert_eq!(
        running.event_type(),
        crate::domains::service::types::ServiceEventType::Started
    );

    let failed = events
        .iter()
        .find(|e| {
            matches!(&e.payload, ServiceEventPayload::StateChanged(s) if s.status == ServiceStatus::Failed)
        })
        .expect("the supervisor's failed reading is reported");
    assert_eq!(
        failed.event_type(),
        crate::domains::service::types::ServiceEventType::Failed
    );

    // The second pull is empty until something changes (and it is always
    // bounded).
    assert!(backend.pull_service_events(sub, u32::MAX).expect("pull").is_empty());
    assert_eq!(backend.pull_service_events(sub, 1).expect("pull").len(), 0);
    let _ = fs::remove_dir_all(&root);
}

/// `control` mints a reference only for a scope the presented capability
/// covers, and every verb on it answers with the contract's honest reason.
#[test]
fn control_mints_a_control_reference_only_for_a_covered_scope() {
    let (backend, root) = hub();
    let all = PresentedCapability {
        id: vec![1u8, 2, 3],
        target: "*".to_string(),
        operations: alloc::vec!["service.control".to_string()],
        expiry_ns: 0,
    };
    let control = backend.control(all).expect("control mints for a wild scope");
    let nm = resolve_id(&backend, "NetworkManager");
    let err = control.clear_metrics(nm).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(err.message.contains("no-backend:"), "{err}");

    // A service the supervisor serves is a covered scope.
    let one = PresentedCapability {
        target: "sshd".to_string(),
        ..Default::default()
    };
    assert!(backend.control(one).is_ok());

    // A target the supervisor does not serve mints nothing.
    let bogus = PresentedCapability {
        target: "telepathy".to_string(),
        ..Default::default()
    };
    let err = match backend.control(bogus) {
        Ok(_) => panic!("a target the supervisor does not serve must not mint a reference"),
        Err(e) => e,
    };
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(err.message.contains("permission-denied:"), "{err}");
    let _ = fs::remove_dir_all(&root);
}

/// The lifecycle verbs speak the supervisor's native control path: the control
/// byte mapping is the supervisor's own protocol, and a host the caller cannot
/// write refuses with the measured class — never by performing a write it was
/// not asked for (charter law #8).

/// `start` speaks the supervisor's own control path — and a control path the
/// caller's credentials do not reach refuses with the measured class, BEFORE
/// any host state is touched (charter law #8).
#[test]
fn start_speaks_the_native_control_path() {
    let (backend, root) = hub();
    assert_eq!(control_byte(LifecycleVerb::Start), b'u');
    let err = backend
        .apply_lifecycle(
            resolve_id(&backend, "NetworkManager"),
            "NetworkManager",
            LifecycleVerb::Start,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(err.message.starts_with("no-privilege:"), "{err}");
    assert!(err.message.contains("control"), "the reason names the path: {err}");
    let _ = fs::remove_dir_all(&root);
}

/// `stop` speaks the supervisor's own control path, and refuses the same way.
#[test]
fn stop_speaks_the_native_control_path() {
    let (backend, root) = hub();
    assert_eq!(control_byte(LifecycleVerb::Stop), b'd');
    let err = backend
        .apply_lifecycle(
            resolve_id(&backend, "NetworkManager"),
            "NetworkManager",
            LifecycleVerb::Stop,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(err.message.starts_with("no-privilege:"), "{err}");
    let _ = fs::remove_dir_all(&root);
}

/// `restart` speaks the supervisor's own control path, and refuses the same
/// way.
#[test]
fn restart_speaks_the_native_control_path() {
    let (backend, root) = hub();
    assert_eq!(control_byte(LifecycleVerb::Restart), b't');
    let err = backend
        .apply_lifecycle(
            resolve_id(&backend, "NetworkManager"),
            "NetworkManager",
            LifecycleVerb::Restart,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(err.message.starts_with("no-privilege:"), "{err}");
    let _ = fs::remove_dir_all(&root);
}

/// The native control write path is proven hermetically: a real FIFO with a
/// reader on the other end receives the supervisor's own byte, and a verb whose
/// FIFO is missing reports the measured error.
#[test]
fn the_control_byte_writes_through_a_real_fifo() {
    let root = temp_root("runit-fifo");
    let _ = fs::remove_dir_all(&root);
    let state = root.join("run/runit");
    let dir = state.join("supervise.scripted");
    fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("control");
    // The supervisor's control path is a fifo; std's mkfifo is unstable on
    // this toolchain, so the test makes one the way the platform does.
    let c_path = alloc::ffi::CString::new(fifo.to_string_lossy().into_owned()).unwrap();
    let made = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    assert_eq!(made, 0, "mkfifo: {}", std::io::Error::last_os_error());
    let scan = root.join("var/service");
    let declared = root.join("etc/sv");
    fs::create_dir_all(&scan).unwrap();
    fs::create_dir_all(&declared).unwrap();
    fs::create_dir_all(declared.join("scripted")).unwrap();
    std::os::unix::fs::symlink(declared.join("scripted"), scan.join("scripted")).unwrap();

    let backend = RunitBackend::with_roots(
        scan,
        declared,
        state,
        root.join("no-proc"),
        root.join("no-pidfiles"),
    );
    // The reader lives in a thread, so the write is not a host daemon: the
    // FIFO is this test's own temp file.
    let reader = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut file = fs::File::open(&fifo).expect("open the fifo for reading");
            let mut byte = [0u8; 1];
            file.read_exact(&mut byte).expect("read one control byte");
            byte[0]
        })
    };
    NativeSupervision::control(&backend, "scripted", LifecycleVerb::Start)
        .expect("the control byte is written through the fifo");
    assert_eq!(reader.join().expect("the reader finishes"), b'u');

    // A service with no control fifo reports the measured error, and the
    // message names the path it could not open.
    let err = NativeSupervision::control(&backend, "absent", LifecycleVerb::Start)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(err.message.contains("supervise.absent/control"), "{err}");
    let _ = fs::remove_dir_all(&root);
}

/// A backend that reached no supervisor answers honestly: every read carries
/// the reason naming what was looked for, and the identity is the empty one.
#[test]
fn no_supervisor_is_an_honest_absence() {
    let backend = LinuxServiceBackend::with_cell(Cell::None);
    assert_eq!(backend.supervisor().supervisor, "");
    let err = backend
        .list_services(Pagination { offset: 0, limit: u32::MAX })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(err.message.starts_with("no-backend:"), "{err}");
    assert!(err.message.contains("/run/runit"), "the reason names the markers: {err}");
    assert!(backend.list_quarantined(Pagination::default()).is_ok());
    // The control accessor refuses to mint an authority it cannot bind.
    assert!(backend
        .control(PresentedCapability { target: "*".into(), ..Default::default() })
        .is_err());
    // And the lifecycle verbs refuse rather than touching anything.
    assert!(backend
        .apply_lifecycle(ServiceId::new(1), "anything", LifecycleVerb::Start)
        .is_err());
}

/// Every availability claim names a test that exists: a REAL/DEGRADED row
/// whose test has vanished is a FAIL, not a silent claim.
///
/// The suite is read from the cell's OWN source at compile time — never from
/// a hand-kept list, which is a list that can drift away from the truth.
#[test]
fn every_claimed_test_exists() {
    for (method, test) in claimed_l3_tests() {
        let name = test.rsplit("::").next().unwrap_or(test);
        assert!(
            the_cells_own_test_names().contains(&name),
            "{method} claims {test}, which is not a test in the service cell"
        );
    }
}

/// Every test function the service cell defines, read from its own source.
fn the_cells_own_test_names() -> Vec<&'static str> {
    let sources = [
        include_str!("tests.rs"),
        include_str!("openrc.rs"),
        include_str!("systemd.rs"),
    ];
    let mut names = Vec::new();
    for source in sources {
        let lines: Vec<&str> = source.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if line.trim() != "#[test]" {
                continue;
            }
            let Some(next) = lines.get(index + 1) else { break };
            let trimmed = next.trim();
            let Some(rest) = trimmed.strip_prefix("fn ") else { continue };
            let name: alloc::string::String =
                rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if !name.is_empty() {
                names.push(leak(name));
            }
        }
    }
    names
}

/// The names are read at compile time, so they live for the whole run.
fn leak(name: alloc::string::String) -> &'static str {
    alloc::boxed::Box::leak(name.into_boxed_str())
}

/// The READ-ONLY live probe (charter law #8): read THIS host's runit supervision
/// tree natively — the declared services, the linked ones, the supervised ones —
/// and read real services' status through the supervisor's own grammar. It
/// writes nothing; the control verbs are founder-run. `#[ignore]`d so CI never
/// depends on a host; run with `--ignored --nocapture` on a runit box.
#[test]
#[ignore = "reads the host's runit supervision tree; read-only, founder-runnable"]
fn live_reads_this_hosts_runit_supervision_natively() {
    let backend = LinuxServiceBackend::native_connect();
    let identity = backend.supervisor();
    std::eprintln!(
        "live supervisor: {} (scan {} declared {} state {})",
        identity.supervisor, identity.service_root, identity.declared_root, identity.state_root
    );
    let caps = backend.supervision_caps();
    std::eprintln!(
        "live caps: enumerate={} read_state={} control={}",
        caps.can_enumerate, caps.can_read_state, caps.can_control
    );
    let page = match backend.list_services(Pagination { offset: 0, limit: u32::MAX }) {
        Ok(page) => page,
        Err(e) => {
            std::eprintln!("live listServices: {e}");
            return;
        }
    };
    std::eprintln!("live services: {} enumerated", page.total);
    for info in page.items.iter() {
        let meta = |key: &str| {
            info.state
                .metadata
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
                .unwrap_or("-")
        };
        std::eprintln!(
            "  {:<24} {:<8} pid={:<7} enabled={:<5} supervised={:<5} runsv={:<6} source={:<14} {}",
            info.name,
            info.state.status.as_token(),
            info.state.main_pid,
            info.enabled,
            meta("supervised"),
            meta("runsv"),
            meta("state_source"),
            if info.state.reason.is_empty() { "-" } else { &info.state.reason }
        );
    }
    for name in ["sshd", "NetworkManager"] {
        let (id, present) = backend.resolve(name).expect("resolve");
        if !present {
            std::eprintln!("live {name}: not linked, not supervised");
            continue;
        }
        match backend.get_state(id) {
            Ok(state) => std::eprintln!(
                "live {name}: {} pid={} reason={}",
                state.status.as_token(),
                state.main_pid,
                if state.reason.is_empty() { "-" } else { &state.reason }
            ),
            Err(e) => std::eprintln!("live {name}: {e}"),
        }
    }
}



