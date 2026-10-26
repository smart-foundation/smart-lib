//! Where the service is, and how the app admits it does not know.
//!
//! There is no registry file in the contract yet: a client finds a service by
//! dialling a carrier the two sides agreed on out of band. That means the
//! client's discovery story is a **candidate list**, and the only honest way
//! to present a candidate list is to show it. So this module returns not "the
//! path" but every path, in order, with where each one came from — and the
//! connection panel renders the list with the outcome of each attempt.
//!
//! Order, most authoritative first:
//!
//! 1. `--socket <path>` on this process's own command line;
//! 2. `$SMART_SERVICE_SOCKET` — the variable the daemon documents, and the one
//!    that makes a per-user or per-container daemon possible;
//! 3. `$XDG_RUNTIME_DIR/smart/service.sock` — the per-user convention;
//! 4. `/tmp/smart-service.sock` — the unix transport's published default
//!    address (`src/transport/unix.rs`), so a stock daemon started with no
//!    arguments is found with no configuration.
//!
//! Steps 3 and 4 are *candidates*, not promises: if the daemon is somewhere
//! else the panel says so, with the list, and `--socket` fixes it in one flag.
//! A future contract revision that publishes an endpoint registry makes this
//! module smaller, not larger — which is the point of keeping the guessing in
//! exactly one place.

use std::path::PathBuf;

/// Where one candidate came from. Shown to the user, because "which of these
/// four did you try?" is the first question anyone asks when it fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointSource {
    /// `--socket` on the command line.
    Argument,
    /// `$SMART_SERVICE_SOCKET`.
    Environment,
    /// `$XDG_RUNTIME_DIR/smart/service.sock`.
    XdgRuntime,
    /// The unix transport's published default address.
    TransportDefault,
}

impl EndpointSource {
    /// The label the connection panel prints.
    pub fn label(self) -> &'static str {
        match self {
            EndpointSource::Argument => "--socket",
            EndpointSource::Environment => "$SMART_SERVICE_SOCKET",
            EndpointSource::XdgRuntime => "$XDG_RUNTIME_DIR/smart/service.sock",
            EndpointSource::TransportDefault => "transport default",
        }
    }
}

/// One place the app will look for the service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The carrier scheme. This app speaks `unix` only; a candidate naming
    /// another scheme is reported as unsupported rather than silently dialled
    /// with the wrong transport.
    pub scheme: String,
    /// The address, in the carrier's own vocabulary.
    pub address: String,
    /// Where this candidate came from.
    pub source: EndpointSource,
}

impl Candidate {
    /// How the panel renders one row before an attempt is made.
    pub fn display(&self) -> String {
        format!("{}://{}", self.scheme, self.address)
    }
}

/// The resolved search plan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceLocation {
    /// Candidates in the order they will be tried. Empty only when the caller
    /// built the plan with no inputs at all, which the UI treats as "nothing
    /// configured" rather than as a failure.
    pub candidates: Vec<Candidate>,
}

impl ServiceLocation {
    /// Build the plan from this process's arguments and environment.
    ///
    /// `args` is the raw `std::env::args()` tail: only `--socket` and
    /// `--addr` are read, and an unknown flag is *not* an error here (the
    /// binary's own parser owns its command line; this module owns only the
    /// endpoint question).
    pub fn from_environment(args: &[String]) -> Self {
        Self::resolve(args, |key| std::env::var(key).ok())
    }

    /// The same plan, with the environment injected. Tests use this; so does
    /// any future embedder that does not want to read the process's globals.
    pub fn resolve(args: &[String], env: impl Fn(&str) -> Option<String>) -> Self {
        let mut candidates: Vec<Candidate> = Vec::new();

        if let Some(address) = flag_value(args, "--socket").or_else(|| flag_value(args, "--addr")) {
            candidates.push(Candidate {
                scheme: "unix".to_string(),
                address,
                source: EndpointSource::Argument,
            });
        }

        if let Some(address) = env("SMART_SERVICE_SOCKET").filter(|value| !value.is_empty()) {
            push_unique(
                &mut candidates,
                Candidate {
                    scheme: "unix".to_string(),
                    address,
                    source: EndpointSource::Environment,
                },
            );
        }

        if let Some(runtime) = env("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()) {
            let address = PathBuf::from(runtime).join("smart").join("service.sock");
            push_unique(
                &mut candidates,
                Candidate {
                    scheme: "unix".to_string(),
                    address: address.display().to_string(),
                    source: EndpointSource::XdgRuntime,
                },
            );
        }

        push_unique(
            &mut candidates,
            Candidate {
                scheme: "unix".to_string(),
                address: "/tmp/smart-service.sock".to_string(),
                source: EndpointSource::TransportDefault,
            },
        );

        ServiceLocation { candidates }
    }

    /// The first candidate, for the common case where the UI just wants a
    /// name to show before anything has been tried.
    pub fn preferred(&self) -> Option<&Candidate> {
        self.candidates.first()
    }
}

/// Read `--flag value` (or `--flag=value`) out of an argument tail.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut iterator = args.iter();
    while let Some(argument) = iterator.next() {
        if argument == flag {
            return iterator.next().cloned();
        }
        if let Some(rest) = argument.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

/// Add a candidate unless an earlier (higher-priority) source already named
/// the same address. Duplicates are noise in an error list.
fn push_unique(candidates: &mut Vec<Candidate>, candidate: Candidate) {
    if candidates
        .iter()
        .any(|existing| existing.address == candidate.address)
    {
        return;
    }
    candidates.push(candidate);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn the_flag_wins_and_is_tried_first() {
        let plan = ServiceLocation::resolve(
            &args(&["--socket", "/run/user/1000/smart.sock"]),
            |key| match key {
                "SMART_SERVICE_SOCKET" => Some("/tmp/other.sock".to_string()),
                _ => None,
            },
        );
        assert_eq!(plan.candidates[0].address, "/run/user/1000/smart.sock");
        assert_eq!(plan.candidates[0].source, EndpointSource::Argument);
        assert_eq!(plan.candidates[1].source, EndpointSource::Environment);
    }

    #[test]
    fn the_default_is_always_last_so_a_stock_daemon_is_found() {
        let plan = ServiceLocation::resolve(&[], |_| None);
        assert_eq!(plan.candidates.len(), 1);
        assert_eq!(plan.candidates[0].address, "/tmp/smart-service.sock");
        assert_eq!(
            plan.candidates[0].source,
            EndpointSource::TransportDefault
        );
    }

    #[test]
    fn an_address_named_twice_is_tried_once() {
        let plan = ServiceLocation::resolve(
            &args(&["--socket", "/tmp/smart-service.sock"]),
            |_| None,
        );
        assert_eq!(plan.candidates.len(), 1);
    }

    #[test]
    fn an_empty_environment_variable_is_not_a_candidate() {
        let plan = ServiceLocation::resolve(&[], |key| match key {
            "SMART_SERVICE_SOCKET" => Some(String::new()),
            _ => None,
        });
        assert_eq!(plan.candidates.len(), 1);
        assert_eq!(
            plan.candidates[0].source,
            EndpointSource::TransportDefault
        );
    }

    #[test]
    fn the_flag_accepts_the_equals_form() {
        let plan = ServiceLocation::resolve(&args(&["--socket=/run/x.sock"]), |_| None);
        assert_eq!(plan.candidates[0].address, "/run/x.sock");
    }
}
