//! The Linux **service** backend's openrc cell — openrc's own state, read
//! natively (the "innovation like netlink" mandate, third supervisor).
//!
//! ## The native decision
//!
//! The archive body ran `rc-service <name> status`, `openrc` and `ls
//! /etc/init.d` as subprocesses and parsed their text. openrc **does** publish
//! its state as files, so this cell reads them directly:
//!
//! | question | native read |
//! |---|---|
//! | what services exist? | `read_dir(/etc/init.d)` — the init scripts |
//! | which are started? | `read(/run/openrc/state/started)` and its siblings (`stopped`, `inactive`, `hotplugged`, …) |
//! | which are in which runlevel? | the `read_dir(/etc/runlevels/<level>)` links |
//! | what is a service called? | the init script's own `description=` line |
//! | what does it depend on? | openrc's `rc_depend_in / rc_use` declarations, read from the init script's own text |
//!
//! openrc publishes **no process ids** in its state files (a pidfile is each
//! script's private convention), so `mainPid` is honestly unmeasured here and
//! the metadata says so, rather than a pid this backend invented.
//!
//! ## The control path: a decided honest absence
//!
//! openrc has **no control protocol** comparable to runit's supervise FIFO or
//! systemd's job API: its only control surface is executing `rc-service
//! <name> <verb>` — i.e. the very subprocess shellout this domain exists to
//! replace (P5). Rather than ship an unproven fork-per-verb path, this cell
//! answers `Unavailable`-with-reason naming exactly that, per backend. The
//! same decision applies to sysvinit (see the hub).
//!
//! This cell is not this host's supervisor (measured: `/run/openrc` does not
//! exist here), so it is built and proven over a scripted sysroot — the same
//! honesty the clipboard's Wayland arm had.

extern crate std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::path::PathBuf;

use crate::core::{Error, Result};
use crate::domains::service::backend::LifecycleVerb;
use crate::domains::service::types::{SupervisionCapabilities, SupervisorIdentity};

use super::{NativeDep, NativeObservation, NativeProcFacts, NativeState, NativeSupervision};

/// Where openrc keeps its per-service state files (one file per state, one
/// service name per line inside it).
pub const STATE_ROOT: &str = "/run/openrc/state";
/// Where the init scripts live.
pub const INIT_D_ROOT: &str = "/etc/init.d";
/// Where the runlevel membership links live (`<runlevel>/<service>`).
pub const RUNLEVELS_ROOT: &str = "/etc/runlevels";
/// The runlevel a service is enabled at by default.
pub const DEFAULT_RUNLEVEL: &str = "default";

/// How many bytes of an init script this backend reads. A bound, so an
/// unexpected file cannot be dragged into memory in full.
const MAX_SCRIPT_BYTES: u64 = 64 * 1024;

/// The openrc backend: the host's openrc state, read natively over injectable
/// roots (the tests build a scripted sysroot).
pub struct OpenrcBackend {
    state_root: PathBuf,
    init_root: PathBuf,
    runlevels_root: PathBuf,
}

impl OpenrcBackend {
    /// Construct over explicit roots.
    pub fn with_roots(
        state_root: PathBuf,
        init_root: PathBuf,
        runlevels_root: PathBuf,
    ) -> Self {
        OpenrcBackend { state_root, init_root, runlevels_root }
    }

    /// The host's openrc layout.
    pub fn host() -> Self {
        OpenrcBackend::with_roots(
            PathBuf::from(STATE_ROOT),
            PathBuf::from(INIT_D_ROOT),
            PathBuf::from(RUNLEVELS_ROOT),
        )
    }

    /// The state file contents, keyed by the state word they name.
    fn state_words(&self) -> Vec<(String, Vec<String>)> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.state_root) {
            for entry in entries.flatten() {
                let word = entry.file_name().to_string_lossy().into_owned();
                let names = std::fs::read_to_string(entry.path())
                    .unwrap_or_default()
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
                out.push((word, names));
            }
        }
        out
    }

    /// The init scripts the host declares.
    fn init_scripts(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.init_root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                names.push(name);
            }
        }
        names.sort_unstable();
        names
    }

    /// The services linked into one runlevel (openrc's "enabled").
    fn runlevel_members(&self, level: &str) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.runlevels_root.join(level)) {
            for entry in entries.flatten() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names
    }

    /// The init script's own `description=` line — openrc's scripts declare
    /// their human-readable name, so this is a read rather than a fabrication.
    fn description_of(&self, name: &str) -> String {
        let path = self.init_root.join(name);
        let Ok(meta) = std::fs::metadata(&path) else { return String::new() };
        if meta.len() > MAX_SCRIPT_BYTES {
            return String::new();
        }
        let Ok(text) = std::fs::read_to_string(&path) else { return String::new() };
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("description=") {
                return rest.trim_matches(|c| c == '"' || c == '\'').to_string();
            }
        }
        String::new()
    }

    /// The dependencies the script itself declares, in openrc's own
    /// vocabulary (`rc_depend_in="net ..."` / `rc_use="..."`). A dependency is
    /// satisfied when openrc's own state file says it is started.
    fn dependencies_of(&self, name: &str) -> Vec<NativeDep> {
        let path = self.init_root.join(name);
        let Ok(meta) = std::fs::metadata(&path) else { return Vec::new() };
        if meta.len() > MAX_SCRIPT_BYTES {
            return Vec::new();
        }
        let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
        let started = self
            .state_words()
            .into_iter()
            .find(|(word, _)| word == "started")
            .map(|(_, names)| names)
            .unwrap_or_default();
        let mut deps = Vec::new();
        for line in text.lines() {
            let Some((_, value)) = line.split_once('=') else { continue };
            if !(line.starts_with("rc_depend_in") || line.starts_with("rc_use")) {
                continue;
            }
            for token in value.split_whitespace() {
                let dep = token.trim_matches(|c| c == '"' || c == '\'').to_string();
                if dep.is_empty() || dep == name || dep == "*" {
                    continue;
                }
                if !deps.iter().any(|d: &NativeDep| d.name == dep) {
                    deps.push(NativeDep {
                        satisfied: started.contains(&dep),
                        name: dep,
                    });
                }
            }
        }
        deps
    }
}

impl NativeSupervision for OpenrcBackend {
    fn supervisor(&self) -> SupervisorIdentity {
        SupervisorIdentity {
            supervisor: "openrc".to_string(),
            service_root: self.runlevels_root.display().to_string(),
            declared_root: self.init_root.display().to_string(),
            state_root: self.state_root.display().to_string(),
        }
    }

    fn enumerate(&self) -> Vec<NativeObservation> {
        let state_words = self.state_words();
        let enabled = self.runlevel_members(DEFAULT_RUNLEVEL);
        self.init_scripts()
            .into_iter()
            .map(|name| {
                // The state word the host's own files carry for this service;
                // `None` (with the measured reason) when the state root is not
                // readable or says nothing about it.
                let word = state_words
                    .iter()
                    .find(|(_, names)| names.iter().any(|n| n == &name))
                    .map(|(word, _)| word.clone());
                let state = word.as_deref().map(|word| NativeState {
                    status: status_from_word(word),
                    main_pid: 0,
                    since: 0,
                    since_us: 0,
                    // openrc's state files carry neither a transition time nor
                    // a process id: the observation is the state word alone.
                    since_valid: false,
                    reason: format!("openrc's own state file reports {word}"),
                });
                let state_reason = if word.is_some() {
                    String::new()
                } else {
                    format!(
                        "unavailable: openrc's state files under {} say nothing about this \
                         service (no pid and no transition time are published there either)",
                        self.state_root.display()
                    )
                };
                NativeObservation {
                    declared: true,
                    enabled: enabled.contains(&name),
                    supervised: word.as_deref() == Some("started"),
                    dependencies: self.dependencies_of(&name),
                    state,
                    state_reason,
                    description: self.description_of(&name),
                    name,
                    proc_facts: NativeProcFacts::default(),
                }
            })
            .collect()
    }

    fn dependencies(&self, name: &str) -> Vec<NativeDep> {
        self.dependencies_of(name)
    }

    fn control(&self, name: &str, verb: LifecycleVerb) -> Result<()> {
        // Decided, not forgotten: openrc's only control surface is executing
        // the rc-service script — the fork-per-verb shellout this domain
        // replaces. The honest answer names it.
        Err(Error::not_supported(format!(
            "no-backend: openrc exposes no native control protocol this backend can speak — its \
             only control surface is the `rc-service {name} {verb}` script invocation, and a \
             subprocess per verb is exactly the mechanism this domain replaced. Nothing was \
             changed; the host's state is untouched.",
            name = name,
            verb = verb.as_token()
        )))
    }

    fn measured(&self) -> (SupervisorIdentity, SupervisionCapabilities) {
        let identity = self.supervisor();
        // Measured: the state root is what this backend can actually read here.
        let readable = self.state_root.is_dir();
        let caps = SupervisionCapabilities {
            supervisor: "openrc".to_string(),
            can_enumerate: self.init_root.is_dir(),
            // openrc publishes its own state as files, which is this
            // backend's exact read as well — no privileged half exists.
            can_read_state: readable,
            can_read_exact_state: readable,
            // openrc has no native control protocol (see `control` above), so
            // the claim is the measured absence, never a promise.
            can_control: false,
        };
        (identity, caps)
    }
}

/// openrc's state word, mapped onto the contract's lifecycle.
fn status_from_word(word: &str) -> crate::domains::service::types::ServiceStatus {
    use crate::domains::service::types::ServiceStatus;
    match word {
        "started" => ServiceStatus::Active,
        "stopped" => ServiceStatus::Inactive,
        "inactive" => ServiceStatus::Inactive,
        "hotplugged" => ServiceStatus::Active,
        "failed" => ServiceStatus::Failed,
        // An unrecognised word is the contract's own `unknown`; the caller
        // keeps the file's word in the reason.
        _ => ServiceStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use std::fs;

    /// Build a scripted openrc sysroot: init scripts with openrc's own
    /// declarations, openrc's own state files, and runlevel links.
    fn scripted() -> (OpenrcBackend, PathBuf) {
        // Unique to this test run and this test: the suite runs in parallel and
        // each test owns its own sysroot.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("smart-openrc-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let init = root.join("init.d");
        let state = root.join("openrc/state");
        let levels = root.join("runlevels/default");
        fs::create_dir_all(&init).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&levels).unwrap();

        fs::write(
            init.join("sshd"),
            "#!/sbin/openrc-run\ndescription=\"OpenBSD Secure Shell server\"\nrc_depend_in=\"net\"\n",
        )
        .unwrap();
        fs::write(
            init.join("net"),
            "#!/sbin/openrc-run\ndescription=\"network stack\"\n",
        )
        .unwrap();
        fs::write(
            init.join("udev"),
            "#!/sbin/openrc-run\ndescription=\"device nodes\"\nrc_use=\"net\"\n",
        )
        .unwrap();
        fs::write(state.join("started"), "sshd\nnet\n").unwrap();
        fs::write(state.join("stopped"), "udev\n").unwrap();

        let backend = OpenrcBackend::with_roots(state, init, root.join("runlevels"));
        (backend, root)
    }

    /// The native read path: openrc's own state files become the domain's
    /// observations, with the script's own description and runlevel links.
    #[test]
    fn the_state_files_become_the_domains_observations() {
        let (backend, root) = scripted();
        let observations = backend.enumerate();
        assert_eq!(observations.len(), 3);

        let sshd = observations.iter().find(|o| o.name == "sshd").unwrap();
        assert!(sshd.declared);
        assert!(sshd.supervised, "openrc's started file is the supervision mark");
        assert_eq!(sshd.description, "OpenBSD Secure Shell server");
        let state = sshd.state.as_ref().unwrap();
        assert_eq!(state.status, crate::domains::service::types::ServiceStatus::Active);
        assert!(!state.since_valid, "openrc publishes no transition time");
        assert_eq!(state.main_pid, 0, "openrc publishes no pid");
        assert!(state.reason.contains("openrc's own state file"));

        let udev = observations.iter().find(|o| o.name == "udev").unwrap();
        assert_eq!(
            udev.state.as_ref().unwrap().status,
            crate::domains::service::types::ServiceStatus::Inactive
        );
        assert!(udev.state_reason.is_empty(), "a stopped service has a state, not an error");

        // The enablement is openrc's runlevel link, measured.
        std::fs::create_dir_all(root.join("runlevels/default")).unwrap();
        std::fs::File::create(root.join("runlevels/default/sshd")).unwrap();
        let backend = OpenrcBackend::with_roots(
            root.join("openrc/state"),
            root.join("init.d"),
            root.join("runlevels"),
        );
        let sshd = backend.enumerate().into_iter().find(|o| o.name == "sshd").unwrap();
        assert!(sshd.enabled, "a runlevel link is openrc's enabled");
        let _ = fs::remove_dir_all(&root);
    }

    /// The dependencies are the script's own openrc declarations.
    #[test]
    fn the_dependencies_are_the_scripts_own_declarations() {
        let (backend, root) = scripted();
        let deps = backend.dependencies("sshd");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "net");
        assert!(deps[0].satisfied, "openrc's started file says net is up");
        // A service with no declaration answers empty, not an error.
        assert!(backend.dependencies("net").is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    /// The control path is a decided honest absence, and it never touches the
    /// host: the reason names the shellout this domain replaces.
    #[test]
    fn the_control_path_is_a_decided_honest_absence() {
        let (backend, root) = scripted();
        let err = backend.control("sshd", LifecycleVerb::Start).unwrap_err();
        assert_eq!(err.code, crate::core::ErrorCode::NotSupported);
        assert!(err.message.starts_with("no-backend:"), "{err}");
        assert!(err.message.contains("rc-service sshd start"), "{err}");
        let (_, caps) = backend.measured();
        assert!(!caps.can_control);
        assert!(caps.can_read_state);
        let _ = fs::remove_dir_all(&root);
    }

    /// A missing state root is an honest absence, never a fabricated empty.
    #[test]
    fn an_absent_state_root_is_an_honest_absence() {
        let backend = OpenrcBackend::with_roots(
            std::env::temp_dir().join("smart-openrc-does-not-exist/state"),
            std::env::temp_dir().join("smart-openrc-does-not-exist/init.d"),
            std::env::temp_dir().join("smart-openrc-does-not-exist/runlevels"),
        );
        let observations = backend.enumerate();
        assert!(observations.is_empty(), "no init scripts, no services");
        let (_, caps) = backend.measured();
        assert!(!caps.can_read_state, "measured: the state root is not there");
        assert!(!caps.can_enumerate);
    }
}
