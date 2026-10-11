//! `network_probe` — the network domain's read-only-first proof binary and the
//! seed of the CLI shape for the new verbs.
//!
//! Read operations (`status`, `radios`, `networking`) run by default and are
//! safe to run on any box (law #8: reads are free — they never change host
//! state). The host-mutating verbs (`set-networking`, `set-radio`) are the
//! founder's live bring-back vehicle and are REFUSED unless the operator passes
//! `--i-know-this-changes-host-state`. The live write proof is founder-run
//! (charter law #8); this binary just wires the backend to a CLI.
//!
//! ```text
//! network_probe                       # read-only: status + radios summary
//! network_probe status [iface]        # per-interface status incl. hostState
//! network_probe radios                # isNetworkingEnabled + listRadioStates
//! network_probe networking            # isNetworkingEnabled
//! network_probe set-networking true   # REQUIRES --i-know-this-changes-host-state
//! network_probe set-radio wifi false  # REQUIRES --i-know-this-changes-host-state
//! ```

#![cfg(all(feature = "domain-network", feature = "std"))]

extern crate std;

use std::io::Write as _;
use std::process::ExitCode;

use smart_lib::domains::network::api::Network;
use smart_lib::domains::network::types::Pagination;
use smart_lib::domains::network::NetworkBackend;
use smart_lib::platforms::linux::network::LinuxNetworkBackend;

const MUTATION_FLAG: &str = "--i-know-this-changes-host-state";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let wants_mutation = |cmd: &str| matches!(cmd, "set-networking" | "set-radio");
    let cmd = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "summary".to_string());
    let flag = args.iter().any(|a| a == MUTATION_FLAG);

    if wants_mutation(&cmd) && !flag {
        eprintln!("refusing to run `{cmd}`: it CHANGES host state.");
        eprintln!("Pass {MUTATION_FLAG} once you accept that this flips a real host switch (founder's live bring-back, law #8).");
        return ExitCode::from(2);
    }

    let backend = match LinuxNetworkBackend::connect() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot reach NetworkManager on the system bus: {e}");
            eprintln!("(is org.freedesktop.NetworkManager running? reads only; nothing was changed)");
            return ExitCode::FAILURE;
        }
    };
    let net = Network::new(&backend);
    let out = std::io::stdout();
    let mut w = out.lock();

    match cmd.as_str() {
        "networking" => match net.networking_enabled() {
            Ok(enabled) => {
                let _ = writeln!(w, "networkingEnabled = {enabled}");
            }
            Err(e) => return report(&e),
        },
        "radios" => {
            match net.summary() {
                Ok(s) => {
                    let _ = writeln!(w, "networkingEnabled = {}", s.networking_enabled);
                    for r in &s.radios.items {
                        let _ = writeln!(
                            w,
                            "  radio {:<9} enabled={} hardBlocked={} softBlocked={}",
                            r.name, r.enabled, r.hard_blocked, r.soft_blocked
                        );
                    }
                }
                Err(e) => return report(&e),
            };
        }
        "status" => match backend.get_interfaces(Pagination { offset: 0, limit: u32::MAX }) {
            Ok(page) => {
                for iface in &page.items {
                    let id = iface.id;
                    match net.status(id) {
                        Ok(s) => {
                            let _ = writeln!(
                                w,
                                "{:<16} kind={:<9} connected={:<5} hostState={}",
                                iface.name, s.kind, s.connected, s.host_state.as_token()
                            );
                        }
                        Err(e) => {
                            let _ = writeln!(w, "{:<16} status error: {e}", iface.name);
                        }
                    }
                }
            }
            Err(e) => return report(&e),
        },
        "set-networking" => {
            let value = args
                .iter()
                .find(|a| a.as_str() == "true" || a.as_str() == "false")
                .cloned();
            let enabled = match value.as_deref() {
                Some("true") => true,
                Some("false") => false,
                _ => {
                    eprintln!("set-networking requires `true` or `false`");
                    return ExitCode::from(2);
                }
            };
            eprintln!("LIVE MUTATION: calling NetworkManager Enable({enabled})");
            match backend.set_networking_enabled(enabled) {
                Ok(()) => {
                    let _ = writeln!(w, "setNetworking({enabled}) acknowledged");
                }
                Err(e) => return report(&e),
            }
        }
        "set-radio" => {
            let radio = args
                .iter()
                .find(|a| !a.starts_with("--") && *a != "set-radio" && !matches!(a.as_str(), "true" | "false"))
                .cloned();
            let value = args.iter().find(|a| matches!(a.as_str(), "true" | "false")).cloned();
            let enabled = match value.as_deref() {
                Some("true") => true,
                Some("false") => false,
                _ => {
                    eprintln!("set-radio requires `true` or `false`");
                    return ExitCode::from(2);
                }
            };
            let Some(radio) = radio else {
                eprintln!("set-radio requires a radio token (e.g. wifi)");
                return ExitCode::from(2);
            };
            eprintln!("LIVE MUTATION: setting radio {radio} enabled={enabled}");
            match backend.set_radio_enabled(&radio, enabled) {
                Ok(()) => {
                    let _ = writeln!(w, "setRadioEnabled({radio}, {enabled}) acknowledged");
                }
                Err(e) => return report(&e),
            }
        }
        "summary" | _ => match net.summary() {
            Ok(s) => {
                let _ = writeln!(w, "networkingEnabled = {}", s.networking_enabled);
                for r in &s.radios.items {
                    let _ = writeln!(
                        w,
                        "  radio {:<9} enabled={} hardBlocked={} softBlocked={}",
                        r.name, r.enabled, r.hard_blocked, r.soft_blocked
                    );
                }
            }
            Err(e) => return report(&e),
        },
    }
    ExitCode::SUCCESS
}

fn report(e: &smart_lib::core::Error) -> ExitCode {
    use smart_lib::core::ErrorCode;
    let hint = if e.code == ErrorCode::PermissionDenied {
        " (host authority refused the op — on this box polkitd is absent, so privileged NM calls are denied)"
    } else {
        ""
    };
    eprintln!("error: {e}{hint}");
    ExitCode::FAILURE
}
